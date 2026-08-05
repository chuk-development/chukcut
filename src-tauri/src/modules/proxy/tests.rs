//! End-to-end: a real 4K source, proxied, and the proxy decoded.
//!
//! Every other test in this module is a unit test of a decision, a key or a
//! queue. This one is the only place the whole thing runs, and it exists
//! because the two failure modes that matter are both invisible to unit tests:
//! a proxy that encodes without error and contains nothing, and a proxy that
//! contains frames nobody can decode. Both produce a green suite and a broken
//! editor.
//!
//! The fixture is generated rather than committed — 4K footage does not belong
//! in a repository — and cached under the target directory, so the first run
//! pays for it and later runs do not.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use crate::modules::export::presets::{Fps, Quality};
use crate::modules::export::{HwAccel, MediaWriter, VideoStreamSpec};
use crate::modules::media::VideoDecoder;

use super::cache::{ProxyCache, SourceKey};
use super::decision::decide;
use super::generate;

/// Big enough to be a real 4K decode, short enough that the test is a test.
const FIXTURE_FRAMES: u64 = 30;
const FIXTURE_WIDTH: u32 = 3840;
const FIXTURE_HEIGHT: u32 = 2160;

/// Where generated fixtures live. Under the build directory rather than the
/// system temp directory so that `cargo clean` disposes of them and a second
/// run of the suite reuses them.
fn fixture_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("proxy-fixtures");
    std::fs::create_dir_all(&dir).expect("fixture directory");
    dir
}

/// A 4K H.264 clip with moving content.
///
/// Moving because a static picture compresses to nothing and decodes in no
/// time, which would make the whole exercise measure the wrong thing.
fn four_k_fixture() -> PathBuf {
    // Three tests want this fixture and `cargo test` runs them in parallel;
    // without the lock two of them race on the same partial file.
    static BUILDING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    // Poison is not interesting here: one test panicking must not turn the
    // other two into a cascade of `PoisonError` that hides the real failure.
    let _guard = BUILDING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let path = fixture_dir().join("4k_h264.mp4");
    if path.exists() {
        return path;
    }

    let spec = VideoStreamSpec {
        width: FIXTURE_WIDTH,
        height: FIXTURE_HEIGHT,
        fps: Fps::THIRTY,
        encoder_name: "libx264".into(),
        accel: HwAccel::Software,
        quality: Quality::Crf(26),
        // The fixture's own encode speed is not what is under test.
        options: vec![("preset".into(), "ultrafast".into())],
    };

    // Same reason as `generate::partial_path`: the muxer is guessed from the
    // extension, so the temporary name keeps `.mp4`.
    let partial = path.with_file_name(".partial-4k_h264.mp4");
    let mut writer = MediaWriter::create(&partial, &spec, None).expect("open the fixture encoder");

    let mut rgba = vec![0u8; (FIXTURE_WIDTH * FIXTURE_HEIGHT * 4) as usize];
    for index in 0..FIXTURE_FRAMES {
        paint(&mut rgba, FIXTURE_WIDTH, FIXTURE_HEIGHT, index);
        writer
            .write_video_frame(&rgba, index)
            .expect("encode a fixture frame");
    }
    writer.finish().expect("finish the fixture");
    std::fs::rename(&partial, &path).expect("name the fixture");
    path
}

/// Smooth gradients plus a moving hard-edged bar.
///
/// The content matters and the obvious choice is wrong. Per-pixel noise —
/// `x ^ y` and friends — is cheap to write and behaves like nothing any camera
/// produces: it does not compress at any resolution, so the *proxy* comes out
/// larger and dearer to decode than the 4K source, and every conclusion drawn
/// from it is backwards. Real footage is locally smooth with a few hard edges,
/// so that is what this paints.
fn paint(rgba: &mut [u8], width: u32, height: u32, index: u64) {
    let bar = ((index * 37) % width as u64) as u32;
    for y in 0..height {
        let row = (y * width * 4) as usize;
        // Vertical component of the gradient, and the moving element that stops
        // the encoder predicting every frame from the last one for nothing.
        let green = ((y * 255) / height.max(1)) as u8;
        for x in 0..width {
            let offset = row + (x * 4) as usize;
            let near_bar = x.abs_diff(bar) < 96;
            rgba[offset] = if near_bar {
                255
            } else {
                ((x * 255) / width.max(1)) as u8
            };
            rgba[offset + 1] = green;
            rgba[offset + 2] = (((x + y) as u64 / 8 + index * 4) % 200) as u8;
            rgba[offset + 3] = 255;
        }
    }
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("chukcut-proxy-e2e-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("scratch");
        Self(path)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The whole point, in one test: a 4K source is judged to need a proxy, the
/// proxy is built, and the result decodes.
#[test]
fn a_four_k_source_is_proxied_and_the_proxy_decodes() {
    let source = four_k_fixture();

    // 1. The rule says yes, without anybody having measured anything.
    let (profile, spec) = generate::plan(&source).expect("plan a proxy");
    assert_eq!(
        (profile.width, profile.height),
        (FIXTURE_WIDTH, FIXTURE_HEIGHT)
    );
    let decision = decide(&profile, None);
    assert!(
        decision.build,
        "a 4K H.264 source must be judged worth proxying — {}",
        decision.reason
    );
    assert_eq!((spec.width, spec.height), (1280, 720));

    // 2. It builds.
    let scratch = Scratch::new("fourk");
    let dest = scratch.path().join("proxy.mp4");
    let cancel = AtomicBool::new(false);
    let generated =
        generate::generate(&source, &dest, &spec, &cancel, &|_, _| {}).expect("build the proxy");

    assert_eq!((generated.width, generated.height), (1280, 720));
    assert!(
        generated.frames >= FIXTURE_FRAMES - 1,
        "the proxy is {} frames against a {FIXTURE_FRAMES}-frame source",
        generated.frames
    );
    assert!(generated.bytes > 0, "the proxy is empty");
    // Deliberately *not* asserting that the proxy is smaller than the source.
    // It is on real footage — measured at 2.9 MB against 5.4 MB for eight
    // seconds of 4K H.264 — but it is not a property of the code, it is a
    // property of the source's bitrate, and this fixture is adversarial: its
    // content is per-pixel noise, which an all-intra encoder cannot compress at
    // any resolution. What the proxy has to be is *cheaper to decode*, and
    // `the_proxy_is_cheaper_to_decode_than_the_source` asserts exactly that.
    eprintln!(
        "4K source {} bytes, 720p proxy {} bytes",
        std::fs::metadata(&source).expect("source").len(),
        generated.bytes
    );
    // And nothing partial is left behind.
    assert!(!dest.with_file_name(".partial-proxy.mp4").exists());

    // 3. It decodes — which is the assertion that catches an encoder that was
    //    never flushed, a muxer whose header was never rewritten, and a file
    //    that probes but contains no decodable picture.
    let probed = crate::modules::media::probe(&dest).expect("probe the proxy");
    let video = probed.video.expect("the proxy has a video stream");
    assert_eq!((video.display_width, video.display_height), (1280, 720));
    assert_eq!(video.codec, "h264");

    let mut decoder = VideoDecoder::open(&dest).expect("open the proxy");
    assert_eq!(decoder.output_size(), (1280, 720));

    // Three points across the clip, including one that requires a seek
    // backwards — the case an all-intra proxy exists to make cheap.
    for at in [0i64, 800_000, 300_000] {
        let frame = decoder.seek_and_decode(at).expect("decode a proxy frame");
        assert_eq!((frame.width, frame.height), (1280, 720));
        assert_eq!(frame.data.len(), 1280 * 720 * 4);
        // A frame of one flat colour is what a broken colour conversion or an
        // unflushed encoder produces. Real content is not flat.
        let first = frame.data[0];
        assert!(
            frame.data.iter().step_by(997).any(|byte| *byte != first),
            "the proxy frame at {at} µs is a flat colour"
        );
    }
}

/// The proxy is cheaper to decode than the source. If it is not, the whole
/// feature is a waste of disk.
#[test]
fn the_proxy_is_cheaper_to_decode_than_the_source() {
    let source = four_k_fixture();
    let scratch = Scratch::new("cost");
    let dest = scratch.path().join("proxy.mp4");

    let (_, spec) = generate::plan(&source).expect("plan");
    let cancel = AtomicBool::new(false);
    generate::generate(&source, &dest, &spec, &cancel, &|_, _| {}).expect("build");

    // Interleaved and taken as the best of two, because this runs on whatever
    // machine the suite runs on and the load can double between one measurement
    // and the next — which is exactly how a real difference turns into a flaky
    // test. Best-of cancels a slow moment; interleaving cancels a slow minute.
    let mut source_ms = f64::INFINITY;
    let mut proxy_ms = f64::INFINITY;
    for _ in 0..2 {
        source_ms = source_ms.min(decode_cost_ms(&source, 1080));
        proxy_ms = proxy_ms.min(decode_cost_ms(&dest, 1080));
    }

    // A wide margin rather than a tight one. The claim being tested is "much
    // cheaper", not a particular number — the real figures are in
    // `docs/STATUS.md` under "Proxy media, measured", where the 4K source is
    // 8–11× the proxy.
    assert!(
        proxy_ms * 2.0 < source_ms,
        "the proxy decodes at {proxy_ms:.1} ms a frame against the source's \
         {source_ms:.1} ms — that is not worth the disk"
    );
    eprintln!("4K source {source_ms:.1} ms/frame, proxy {proxy_ms:.1} ms/frame");
}

/// Decode every frame of `path` at preview scale and return the mean cost.
///
/// The same call `media::provider` makes per frame, so the number means what
/// the preview would experience.
fn decode_cost_ms(path: &Path, target_height: u32) -> f64 {
    let mut decoder = VideoDecoder::open_scaled(path, target_height).expect("open");
    // Discard the first: it pays for the codec's own initialisation.
    let _ = decoder.seek_and_decode(0);

    let fps = Fps::THIRTY;
    let started = std::time::Instant::now();
    let mut decoded = 0u64;
    for index in 1..FIXTURE_FRAMES {
        if decoder.seek_and_decode(fps.frame_time(index)).is_ok() {
            decoded += 1;
        }
    }
    let elapsed = started.elapsed().as_secs_f64() * 1000.0;
    if decoded == 0 {
        return f64::INFINITY;
    }
    elapsed / decoded as f64
}

/// The queue's own path, end to end, against the cache: enqueue, wait, and
/// find the proxy where the preview will look for it.
#[test]
fn the_queue_builds_a_proxy_and_the_cache_serves_it() {
    use super::queue::{EnqueueOutcome, ProxyQueue};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    let source = four_k_fixture();
    let scratch = Scratch::new("queue");
    let cache = Arc::new(ProxyCache::open(scratch.path().join("cache"), 1 << 30));
    let queue =
        ProxyQueue::with_transcoder(Arc::clone(&cache), Arc::new(super::queue::FfmpegTranscoder));

    // Nothing cached: the preview decodes the original, immediately, with no
    // waiting. This is the "never block an import" property.
    let began = Instant::now();
    let outcome = queue.enqueue(&source).expect("enqueue");
    assert!(
        began.elapsed() < Duration::from_secs(2),
        "enqueue took {:?} — an import must not wait for a transcode",
        began.elapsed()
    );
    assert!(
        matches!(&outcome, EnqueueOutcome::Queued { .. }),
        "expected the 4K fixture to be queued, got {outcome:?}"
    );
    assert!(!queue.preview_source(&source).is_proxied());

    // Wait for it.
    let deadline = Instant::now() + Duration::from_secs(120);
    while cache.lookup(&source).is_none() {
        assert!(Instant::now() < deadline, "the proxy never finished");
        std::thread::sleep(Duration::from_millis(50));
    }

    // Now the preview switches, and the export type still cannot be built
    // from what it switched to.
    let preview = queue.preview_source(&source);
    assert!(preview.is_proxied());
    assert_ne!(preview.decode_path(), preview.original());
    assert_eq!(preview.original(), source.as_path());

    let key = SourceKey::of(&source).expect("key");
    assert_eq!(preview.decode_path(), cache.path_for(&key));

    // Asking again is a cache hit rather than a second transcode.
    assert!(matches!(
        queue.enqueue(&source).expect("enqueue"),
        EnqueueOutcome::Cached { .. }
    ));

    queue.shutdown();
}
