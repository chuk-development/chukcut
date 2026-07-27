//! Images and audio as first-class material.
//!
//! Everything here goes through the same functions the app uses — the real
//! FFmpeg probe, `import_material` (the pure half of `project_import_media`),
//! the real `MediaSourceProvider` and the real compositor — with fixtures
//! written by the test itself, so the suite runs anywhere and asserts on
//! content it chose.
//!
//! The refusal cases matter as much as the happy ones. FFmpeg's demuxers are
//! eager: the `tty` demuxer matches a `.txt` file on its extension alone and
//! reports an `ansi` "video" stream, which used to import a text file as a
//! video material whose card could never render. And a file whose extension
//! lies — text bytes wearing `.mp4` — has to fail at probe time with the path
//! in the message, not import as a broken card.

mod support;

use std::path::{Path, PathBuf};

use chukcut_lib::modules::media::{self, thumbnail_strip, MediaSourceProvider};
use chukcut_lib::modules::project::commands::import_material;
use chukcut_lib::modules::project::document::{
    CanvasConfig, MaterialKind, Micros, Project, TimeRange, Track, TrackKind,
};
use chukcut_lib::modules::project::migrate;
use chukcut_lib::modules::render::Compositor;
use chukcut_lib::modules::timeline::ops::EditCommand;
use chukcut_lib::modules::workspace::paths;

use support::assert_pixel_near;

// ---------------------------------------------------------------------------
// Fixtures, written by the test
// ---------------------------------------------------------------------------

/// A file in the system temp dir that cleans up after itself, along with any
/// thumbnail cache the test grew for it.
struct TempFile {
    path: PathBuf,
}

impl TempFile {
    fn bytes(name: &str, bytes: &[u8]) -> Self {
        let path = std::env::temp_dir().join(format!("chukcut-import-{}-{name}", std::process::id()));
        std::fs::write(&path, bytes).expect("write the fixture");
        Self { path }
    }

    /// A solid-colour still, in whatever format the extension of `name` says.
    fn image(name: &str, width: u32, height: u32, rgba: [u8; 4]) -> Self {
        let path = std::env::temp_dir().join(format!("chukcut-import-{}-{name}", std::process::id()));
        // RGB rather than RGBA, because the JPEG encoder refuses an alpha
        // channel; the tests only ever assert on opaque colours anyway.
        let picture =
            image::RgbImage::from_pixel(width, height, image::Rgb([rgba[0], rgba[1], rgba[2]]));
        picture.save(&path).expect("encode the fixture image");
        Self { path }
    }

    /// One second of 16-bit PCM at 8 kHz — the plain 16-byte `fmt ` chunk, no
    /// channel mask, which is the shape most dropped WAVs have.
    fn wav(name: &str) -> Self {
        const RATE: u32 = 8_000;
        let frames = RATE as usize; // exactly one second
        let data_len = frames * 2;

        let mut bytes = Vec::with_capacity(44 + data_len);
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&((36 + data_len) as u32).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes()); // WAVE_FORMAT_PCM
        bytes.extend_from_slice(&1u16.to_le_bytes()); // mono
        bytes.extend_from_slice(&RATE.to_le_bytes());
        bytes.extend_from_slice(&(RATE * 2).to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(data_len as u32).to_le_bytes());
        for frame in 0..frames {
            let value = (frame as f32 * std::f32::consts::TAU * 440.0 / RATE as f32).sin();
            bytes.extend_from_slice(&((value * i16::MAX as f32) as i16).to_le_bytes());
        }
        Self::bytes(name, &bytes)
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn path_str(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_dir_all(paths::thumbnails_dir(&self.path));
    }
}

fn blank_project() -> Project {
    Project::new("import", CanvasConfig::default(), 30.0)
}

/// Probe and import in one move, the way the command does.
fn import(project: &mut Project, file: &TempFile) -> Result<chukcut_lib::modules::project::commands::ImportedMaterial, String> {
    let info = media::probe(file.path()).map_err(|e| e.to_string())?;
    let name = file
        .path()
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    import_material(project, &file.path_str(), &name, &info)
}

// ---------------------------------------------------------------------------
// Images import as image materials
// ---------------------------------------------------------------------------

#[test]
fn a_png_imports_as_an_image_with_its_pixel_size() {
    let file = TempFile::image("size.png", 320, 200, [40, 200, 90, 255]);
    let mut project = blank_project();

    let imported = import(&mut project, &file).expect("a PNG is media");
    assert_eq!(imported.kind, MaterialKind::Image);
    assert_eq!((imported.width, imported.height), (320, 200));
    assert_eq!(imported.duration, 0, "a still has no duration of its own");
    assert!(!imported.has_audio);

    assert_eq!(project.materials.images.len(), 1);
    assert!(project.materials.videos.is_empty(), "a still is not a one-frame video");
    let pooled = &project.materials.images[0];
    assert_eq!((pooled.width, pooled.height), (320, 200));
}

#[test]
fn jpeg_and_webp_import_as_images_too() {
    for name in ["still.jpg", "still.webp"] {
        let file = TempFile::image(name, 64, 48, [200, 40, 40, 255]);
        let mut project = blank_project();
        let imported = import(&mut project, &file)
            .unwrap_or_else(|e| panic!("{name} should import as an image: {e}"));
        assert_eq!(imported.kind, MaterialKind::Image, "{name}");
        assert_eq!((imported.width, imported.height), (64, 48), "{name}");
    }
}

#[test]
fn importing_the_same_image_twice_returns_the_pooled_material() {
    let file = TempFile::image("twice.png", 16, 16, [1, 2, 3, 255]);
    let mut project = blank_project();

    let first = import(&mut project, &file).expect("import");
    let second = import(&mut project, &file).expect("reimport");
    assert_eq!(first.id, second.id);
    assert_eq!(project.materials.images.len(), 1, "no duplicate pool entry");
}

#[test]
fn an_imported_image_survives_save_and_load() {
    let file = TempFile::image("persist.png", 100, 80, [10, 20, 30, 255]);
    let mut project = blank_project();
    let imported = import(&mut project, &file).expect("import");

    let json = serde_json::to_string_pretty(&project).expect("serialize");
    let loaded = migrate::load(&json).expect("a file this build wrote must open");
    assert!(loaded.warnings.is_empty(), "no repairs on a fresh save: {:?}", loaded.warnings);

    assert_eq!(loaded.project.materials.images.len(), 1);
    let reloaded = &loaded.project.materials.images[0];
    assert_eq!(reloaded.id, imported.id);
    assert_eq!(reloaded.path, file.path_str());
    assert_eq!((reloaded.width, reloaded.height), (100, 80));
}

/// The media library asks for a filmstrip the moment a tile exists; for a
/// still that strip is the poster. It goes through the same FFmpeg decode
/// path as video thumbnails, which is exactly why it is worth pinning.
#[test]
fn an_image_gets_thumbnails_for_its_library_card() {
    let file = TempFile::image("poster.png", 64, 48, [90, 90, 220, 255]);
    let tiles = thumbnail_strip(file.path(), 3, 40).expect("a still can be thumbnailed");
    assert_eq!(tiles.len(), 3);
    for tile in &tiles {
        let written = std::fs::metadata(tile).map(|m| m.len()).unwrap_or(0);
        assert!(written > 0, "an empty thumbnail at {tile}");
    }
}

// ---------------------------------------------------------------------------
// A still on the timeline has no source limit
// ---------------------------------------------------------------------------

/// Build a project holding one image clip of `duration`, and hand back the
/// segment id.
fn project_with_still(file: &TempFile, duration: Micros) -> (Project, String) {
    let mut project = blank_project();
    import(&mut project, file).expect("import the still");
    let material_id = project.materials.images[0].id.clone();

    let segment = support::segment(&material_id, 0, duration);
    let segment_id = segment.id.clone();
    let mut track = Track::new(TrackKind::Video, "V1");
    track.segments.push(segment);
    project.tracks.push(track);
    (project, segment_id)
}

#[test]
fn a_still_clip_trims_to_any_length() {
    let file = TempFile::image("trim.png", 32, 32, [255, 255, 0, 255]);
    let five: Micros = 5_000_000;
    let minute: Micros = 60_000_000;
    let (mut project, segment_id) = project_with_still(&file, five);

    // Far beyond the default five seconds: a still has no source duration, so
    // nothing may clamp this.
    EditCommand::TrimSegment {
        segment_id: segment_id.clone(),
        before_target: TimeRange::new(0, five),
        before_source: TimeRange::new(0, five),
        after_target: TimeRange::new(0, minute),
        after_source: TimeRange::new(0, minute),
    }
    .apply(&mut project)
    .expect("a still stretches to any length");
    assert_eq!(project.tracks[0].segments[0].target_range.duration, minute);

    // And back down to a single frame.
    let frame: Micros = 33_333;
    EditCommand::TrimSegment {
        segment_id: segment_id.clone(),
        before_target: TimeRange::new(0, minute),
        before_source: TimeRange::new(0, minute),
        after_target: TimeRange::new(0, frame),
        after_source: TimeRange::new(0, frame),
    }
    .apply(&mut project)
    .expect("a still shrinks to a frame");
    assert_eq!(project.tracks[0].segments[0].target_range.duration, frame);

    // But never to nothing.
    let refused = EditCommand::TrimSegment {
        segment_id,
        before_target: TimeRange::new(0, frame),
        before_source: TimeRange::new(0, frame),
        after_target: TimeRange::new(0, 0),
        after_source: TimeRange::new(0, 0),
    }
    .apply(&mut project);
    assert!(refused.is_err(), "a clip of no length is not a clip");
}

// ---------------------------------------------------------------------------
// A still renders, in pixels
// ---------------------------------------------------------------------------

/// A green square still on a red canvas fills the frame edge to edge, and a
/// wide still on the same canvas is letterboxed with the background showing —
/// the same two facts `tests/compositor.rs` pins for video, through the same
/// provider and compositor, but with the `image` crate doing the decoding.
#[test]
fn a_still_composites_into_the_frame_with_its_own_pixels() {
    let ctx = require_gpu!();

    const GREEN: [u8; 4] = [0, 255, 0, 255];
    const RED: [u8; 4] = [255, 0, 0, 255];
    // A PNG is lossless, so the only slack needed is the sRGB round trip
    // through the render target.
    const TOLERANCE: i32 = 2;

    let matching = TempFile::image("fills.png", 320, 240, GREEN);
    let (mut project, _) = project_with_still(&matching, 5_000_000);
    project.canvas = CanvasConfig {
        width: 320,
        height: 240,
        background: [1.0, 0.0, 0.0, 1.0],
    };

    let compositor = Compositor::new(ctx.clone());
    let sources = MediaSourceProvider::from_project(&project);
    let frame = compositor
        .render(&project, 1_000_000, (320, 240), &sources)
        .expect("render a still");
    assert_pixel_near(frame.pixel(160, 120), GREEN, TOLERANCE, "centre of the still");
    assert_pixel_near(frame.pixel(2, 2), GREEN, TOLERANCE, "corner: the still fills its canvas");

    // Twice as wide as the canvas is tall-for: letterboxed, background above.
    let wide = TempFile::image("wide.png", 640, 240, GREEN);
    let (mut project, _) = project_with_still(&wide, 5_000_000);
    project.canvas = CanvasConfig {
        width: 320,
        height: 240,
        background: [1.0, 0.0, 0.0, 1.0],
    };
    let sources = MediaSourceProvider::from_project(&project);
    let frame = compositor
        .render(&project, 1_000_000, (320, 240), &sources)
        .expect("render the wide still");
    // 640x240 fit into 320 wide is 320x120, centred: rows 60..180.
    assert_pixel_near(frame.pixel(160, 120), GREEN, TOLERANCE, "centre of the letterboxed still");
    assert_pixel_near(frame.pixel(160, 20), RED, TOLERANCE, "letterbox above shows the background");
    assert_pixel_near(frame.pixel(160, 220), RED, TOLERANCE, "letterbox below shows the background");
}

// ---------------------------------------------------------------------------
// Audio imports as audio material
// ---------------------------------------------------------------------------

#[test]
fn a_wav_imports_as_audio_with_its_duration() {
    let file = TempFile::wav("tone.wav");
    let mut project = blank_project();

    let imported = import(&mut project, &file).expect("a WAV is media");
    assert_eq!(imported.kind, MaterialKind::Audio);
    assert!(imported.has_audio);
    // One second of 8 kHz PCM, declared by the header, read by the probe.
    assert!(
        (imported.duration - 1_000_000).abs() < 10_000,
        "duration was {} µs, wanted about a second",
        imported.duration
    );

    assert_eq!(project.materials.audios.len(), 1);
    let pooled = &project.materials.audios[0];
    assert_eq!(pooled.sample_rate, 8_000);
    assert_eq!(pooled.channels, 1);
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// FFmpeg's `tty` demuxer reads any `.txt` as an `ansi` video stream. Without
/// the text-art guard this imported as a video material with a broken card.
#[test]
fn a_text_file_is_refused_with_a_sentence_not_imported_as_video() {
    let file = TempFile::bytes(
        "notes.txt",
        b"these are somebody's notes, not media, and must not become a clip\n".repeat(50).as_slice(),
    );
    let mut project = blank_project();

    let refused = import(&mut project, &file);
    let message = refused.expect_err("a text file is not media");
    assert!(
        message.contains("text file"),
        "the refusal should say what the file is: {message}"
    );
    assert!(project.materials.videos.is_empty());
    assert!(project.materials.images.is_empty());
    assert!(project.materials.audios.is_empty());
}

#[test]
fn a_text_file_wearing_a_video_extension_fails_at_probe_time() {
    let file = TempFile::bytes(
        "lies.mp4",
        b"this is prose pretending to be an MP4 container\n".repeat(100).as_slice(),
    );
    let error = media::probe(file.path()).expect_err("text is not an MP4");
    let message = error.to_string();
    assert!(
        message.contains("lies.mp4"),
        "the error should name the file: {message}"
    );
}

#[test]
fn random_bytes_wearing_a_video_extension_fail_at_probe_time() {
    // Deterministic garbage: an LCG, so a failure reproduces.
    let mut state: u64 = 0x2545_F491_4F6C_DD1D;
    let bytes: Vec<u8> = (0..64 * 1024)
        .map(|_| {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (state >> 56) as u8
        })
        .collect();
    let file = TempFile::bytes("garbage.mp4", &bytes);
    assert!(
        media::probe(file.path()).is_err(),
        "64 KB of noise is not a video"
    );
}

/// A container FFmpeg can open but which holds nothing usable — a subtitle
/// file — is refused with the streams named, not imported as an empty card.
#[test]
fn a_subtitle_file_is_refused_as_holding_no_media_stream() {
    let file = TempFile::bytes(
        "captions.srt",
        b"1\n00:00:00,000 --> 00:00:02,000\nHello there\n\n2\n00:00:02,000 --> 00:00:04,000\nStill no pixels\n",
    );
    let mut project = blank_project();

    // Some FFmpeg builds refuse the probe outright, others open it and report
    // no audio or video stream. Either way nothing may land in the pool.
    if let Ok(info) = media::probe(file.path()) {
        let refused = import_material(&mut project, &file.path_str(), "captions.srt", &info);
        let message = refused.expect_err("subtitles are not media");
        assert!(
            message.contains("no video, image or audio stream"),
            "the refusal should say what was looked for: {message}"
        );
    }
    assert!(project.materials.videos.is_empty());
    assert!(project.materials.audios.is_empty());
    assert!(project.materials.images.is_empty());
}
