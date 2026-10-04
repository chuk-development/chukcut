//! Compound clips render, mix and export like the clips they hold.
//!
//! One document, two shapes: the clips laid out on the timeline, and the
//! same clips inside a compound clip — trimmed, moved and nested again — with
//! `sequence::build::flatten` producing the laid-out shape from the nested
//! one. Every frame the compositor draws of the two must agree, the export of
//! the two must agree, and so must the mix.

mod support;

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use chukcut_engine::modules::export::presets::Container;
use chukcut_engine::modules::export::{
    mix_timeline, resolve_settings, run_export, ExportJob, ExportOverrides, ExportRequest, FnSink,
};
use chukcut_engine::modules::media::{MediaSourceProvider, VideoDecoder};
use chukcut_engine::modules::project::document::{
    AudioMaterial, CanvasConfig, Micros, Project, Severity, TimeRange, Track, TrackKind,
};
use chukcut_engine::modules::render::{Compositor, CompositorConfig, SourceProvider};
use chukcut_engine::modules::sequence::build;
use chukcut_engine::modules::timeline::ops::EditCommand;

use support::{material_for, read_counter_rgba, segment};

const S: Micros = 1_000_000;
const W: u32 = 320;
const H: u32 = 240;

/// The counter and the quadrants on the main lane, a half-transparent,
/// half-size, off-centre white clip above them, and a sine below.
fn laid_out() -> Option<Project> {
    let media = support::media().ok()?;
    let mut p = Project::new(
        "compound",
        CanvasConfig {
            width: W,
            height: H,
            background: [0.0, 0.0, 0.0, 1.0],
        },
        30.0,
    );
    p.materials
        .videos
        .push(material_for("counter", &media.counter).ok()?);
    p.materials
        .videos
        .push(material_for("quadrants", &media.quadrants).ok()?);
    p.materials
        .videos
        .push(material_for("white", &media.solid_white_wide).ok()?);
    let sine = chukcut_engine::modules::media::probe(&media.audio_only).ok()?;
    p.materials.audios.push(AudioMaterial {
        id: "sine".into(),
        path: media.audio_only.to_string_lossy().into_owned(),
        duration: sine.duration,
        sample_rate: 48_000,
        channels: 1,
    });

    let mut main = Track::new(TrackKind::Video, "Video 1");
    let mut counter = segment("counter", 0, S);
    counter.source_range = TimeRange::new(S / 2, S);
    main.segments.push(counter);
    main.segments.push(segment("quadrants", S, S));
    let mut overlay = Track::new(TrackKind::Video, "Video 2");
    let mut white = segment("white", S / 2, S);
    white.transform.opacity = 0.5;
    white.transform.scale = [0.5, 0.5];
    white.transform.position = [0.3, -0.2];
    overlay.segments.push(white);
    let mut sound = Track::new(TrackKind::Audio, "Audio 1");
    let mut sine_clip = segment("sine", 0, 2 * S);
    sine_clip.volume = 0.7;
    sound.segments.push(sine_clip);
    p.tracks = vec![main, overlay, sound];
    for (i, t) in p.tracks.iter_mut().enumerate() {
        for s in &mut t.segments {
            s.render_index = i as i32;
        }
    }
    Some(p)
}

fn apply(p: &mut Project, command: EditCommand) {
    command.apply(p).expect("the edit applies");
    let errors: Vec<String> = p
        .validate()
        .into_iter()
        .filter(|i| i.severity == Severity::Error)
        .map(|i| i.message)
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
}

fn all_clips(p: &Project) -> Vec<String> {
    p.tracks
        .iter()
        .flat_map(|t| t.segments.iter().map(|s| s.id.clone()))
        .collect()
}

/// Everything into one compound clip, which is then itself put inside a
/// second, trimmed to show 0.25..1.75 s of its contents, and moved to 0.5 s.
/// Answers the nested project and the id of the outer compound clip.
fn nested(original: &Project) -> (Project, String) {
    let mut p = original.clone();
    let inner = build::create_compound(&p, &all_clips(&p), None).unwrap();
    apply(&mut p, inner.command);
    let outer = build::create_compound(&p, &[inner.segment_id], None).unwrap();
    apply(&mut p, outer.command);
    let (lane, clip) = p.segment(&outer.segment_id).unwrap();
    let (lane, clip) = (lane.id.clone(), clip.clone());
    apply(
        &mut p,
        EditCommand::TrimSegment {
            segment_id: clip.id.clone(),
            before_target: clip.target_range,
            before_source: clip.source_range,
            after_target: TimeRange::new(S / 4, 3 * S / 2),
            after_source: TimeRange::new(S / 4, 3 * S / 2),
        },
    );
    apply(
        &mut p,
        EditCommand::MoveSegment {
            segment_id: clip.id.clone(),
            from_track: lane.clone(),
            to_track: lane,
            from_start: S / 4,
            to_start: S / 2,
        },
    );
    (p, outer.segment_id)
}

/// The nested project with its compound clips put back, outer then inner.
fn flattened(nested: &Project, outer: &str) -> Project {
    let mut p = nested.clone();
    let c = build::flatten(&p, outer).unwrap();
    apply(&mut p, c);
    let inner = p
        .tracks
        .iter()
        .flat_map(|t| t.segments.iter())
        .find(|s| p.materials.sequence(&s.material_id).is_some())
        .map(|s| s.id.clone())
        .expect("the inner compound clip came back out");
    let c = build::flatten(&p, &inner).unwrap();
    apply(&mut p, c);
    assert!(p.materials.sequences.is_empty());
    p
}

fn render(c: &Compositor, p: &Project, at: Micros) -> Vec<u8> {
    let sources = MediaSourceProvider::from_project(p);
    c.render(p, at, (W, H), &sources).expect("render").data
}

fn max_difference(a: &[u8], b: &[u8]) -> (u8, usize) {
    let mut worst = (0u8, 0usize);
    for (i, (x, y)) in a.iter().zip(b).enumerate() {
        let d = x.abs_diff(*y);
        if d > worst.0 {
            worst = (d, i / 4);
        }
    }
    worst
}

#[test]
fn a_nested_compound_clip_draws_the_frames_its_flattened_clips_draw() {
    let _ = require_media!();
    let ctx = require_gpu!();
    let Some(original) = laid_out() else {
        return;
    };
    let (nested, outer) = nested(&original);
    let flat = flattened(&nested, &outer);
    assert_eq!(nested.duration(), flat.duration());
    let c = Compositor::with_config(
        ctx,
        CompositorConfig {
            strict_sources: true,
            ..Default::default()
        },
    );
    // Every frame across the cut, the overlay's edges and the window's ends.
    let mut at = 0;
    while at < nested.duration() {
        let a = render(&c, &nested, at + 10);
        let b = render(&c, &flat, at + 10);
        let (worst, pixel) = max_difference(&a, &b);
        assert!(
            worst <= 2,
            "at {at} µs the nested frame differs by {worst} at pixel ({}, {})",
            pixel as u32 % W,
            pixel as u32 / W
        );
        at += S / 10;
    }
    // And the counter in it is the right frame: 0.5 s into the timeline is
    // 0.25 s into the compound clip, the counter's source 0.75 s.
    let frame = render(&c, &nested, S / 2 + 10);
    assert_eq!(read_counter_rgba(&frame, W, H), Some(22));
}

#[test]
fn a_half_transparent_compound_clip_composites_like_its_contents() {
    let _ = require_media!();
    let ctx = require_gpu!();
    let Some(original) = laid_out() else {
        return;
    };
    // Only the overlay goes in: the compound clip is mostly transparent and,
    // where it is not, half so — the case premultiplied colour gets wrong.
    let white = original.tracks[1].segments[0].id.clone();
    let mut nested = original.clone();
    let made = build::create_compound(&nested, &[white], None).unwrap();
    apply(&mut nested, made.command);
    let c = Compositor::new(ctx);
    for at in [S / 2 + 10, S + 10, 3 * S / 2 - 10] {
        let a = render(&c, &nested, at);
        let b = render(&c, &original, at);
        let (worst, pixel) = max_difference(&a, &b);
        assert!(
            worst <= 2,
            "at {at} µs: off by {worst} at pixel ({}, {})",
            pixel as u32 % W,
            pixel as u32 / W
        );
    }
}

#[test]
fn a_nested_project_mixes_the_same_sound() {
    let _ = require_media!();
    let Some(original) = laid_out() else {
        return;
    };
    let (nested, outer) = nested(&original);
    let flat = flattened(&nested, &outer);
    let cancel = AtomicBool::new(false);
    let source: Arc<dyn chukcut_engine::modules::export::AudioSource> =
        Arc::new(chukcut_engine::modules::audio::FileAudioSource);
    let a = mix_timeline(&nested, source.as_ref(), 48_000, 2, &cancel).unwrap();
    let b = mix_timeline(&flat, source.as_ref(), 48_000, 2, &cancel).unwrap();
    assert_eq!(a.len(), b.len());
    let worst = a
        .iter()
        .zip(&b)
        .map(|(x, y)| (x - y).abs())
        .fold(0.0f32, f32::max);
    assert!(worst < 1e-4, "the mixes differ by {worst}");
    assert!(
        a.iter().any(|s| s.abs() > 0.05),
        "and there is sound in them"
    );
}

fn export(project: &Project, name: &str) -> Option<std::path::PathBuf> {
    let ctx = support::gpu()?;
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("compound");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(name);
    let _ = std::fs::remove_file(&path);
    let request = ExportRequest {
        output_path: path.to_string_lossy().into_owned(),
        preset_id: None,
        overrides: Some(ExportOverrides {
            fps: Some(30.0),
            container: Some(Container::Mp4),
            ..Default::default()
        }),
        hardware: None,
        include_audio: true,
        range: None,
    };
    let settings = resolve_settings(project, &request).expect("resolve the settings");
    let job = ExportJob {
        job_id: name.into(),
        project: project.clone(),
        settings,
        compositor: Arc::new(Compositor::with_config(
            ctx,
            CompositorConfig {
                strict_sources: true,
                ..Default::default()
            },
        )),
        sources: Arc::new(MediaSourceProvider::from_project(project)) as Arc<dyn SourceProvider>,
        audio: Arc::new(chukcut_engine::modules::audio::FileAudioSource),
        cancel: Arc::new(AtomicBool::new(false)),
    };
    let outcome = run_export(&job, &FnSink(|_| {})).expect("the export finishes");
    assert!(!outcome.cancelled);
    Some(path)
}

#[test]
fn exporting_a_nested_project_matches_exporting_it_flattened() {
    let _ = require_media!();
    let _ = require_gpu!();
    let Some(original) = laid_out() else {
        return;
    };
    let (nested, outer) = nested(&original);
    let flat = flattened(&nested, &outer);
    let (Some(a), Some(b)) = (
        export(&nested, "nested.mp4"),
        export(&flat, "flattened.mp4"),
    ) else {
        return;
    };
    let probe_a = support::probe_output(&a).unwrap();
    let probe_b = support::probe_output(&b).unwrap();
    assert_eq!(probe_a.decoded_frames, probe_b.decoded_frames);
    assert_eq!(probe_a.decoded_frames, 60, "two seconds at 30 fps");
    let mut da = VideoDecoder::open(&a).unwrap();
    let mut db = VideoDecoder::open(&b).unwrap();
    for n in [0u64, 10, 15, 20, 29, 30, 44, 59] {
        let at = (n as f64 * S as f64 / 30.0).round() as Micros + 16_000;
        let fa = da.seek_and_decode(at).unwrap();
        let fb = db.seek_and_decode(at).unwrap();
        let (worst, _) = max_difference(&fa.data, &fb.data);
        // The same encoder on the same pictures: identical up to the odd
        // rounding the nested render's extra 8-bit store can introduce.
        assert!(worst <= 6, "frame {n} differs by {worst}");
    }
    let _ = std::fs::remove_file(&a);
    let _ = std::fs::remove_file(&b);
}

#[test]
fn the_export_of_an_open_compound_clip_is_the_whole_timeline() {
    let Some(original) = laid_out() else {
        return;
    };
    let (mut nested, outer) = nested(&original);
    let c = build::open(&nested, &outer).unwrap();
    apply(&mut nested, c);
    assert_ne!(nested.sequence.id, "main");
    // Inside: the inner compound clip, 0..2 s. Outside: the outer one, moved
    // to 0.5 s and showing 1.5 s.
    assert_eq!(nested.tracks.len(), 1);
    let root = chukcut_engine::modules::sequence::export_root(nested.clone());
    assert_eq!(root.sequence.id, "main");
    assert_eq!(root.duration(), 2 * S);
    let clip = &root.tracks[0].segments[0];
    assert_eq!(clip.target_range, TimeRange::new(S / 2, 3 * S / 2));
}

/// One compositor and one provider across renders, the way the preview keeps
/// them: the same instant twice is the cached nested frame, and an edit
/// inside the compound clip — to a clip, or to a pool entry a clip names —
/// is never served stale.
#[test]
fn the_nested_frame_cache_reuses_and_refreshes() {
    let _ = require_media!();
    let ctx = require_gpu!();
    let Some(mut original) = laid_out() else {
        return;
    };
    // A grade on the counter, so an edit to the pool entry can be checked.
    original.materials.color_adjusts.push(
        chukcut_engine::modules::project::document::ColorAdjustMaterial {
            id: "look".into(),
            brightness: 0.0,
            contrast: 1.0,
            saturation: 1.0,
            temperature: 0.0,
            lut: None,
            grade: Default::default(),
        },
    );
    original.tracks[0].segments[0].extras.push("look".into());
    let (mut nested, _) = nested(&original);
    // 0.75 s into the compound clip's contents: the overlay is on screen.
    let at = S + 10;
    let c = Compositor::new(Arc::clone(&ctx));
    let sources = MediaSourceProvider::from_project(&nested);
    let draw = |c: &Compositor, p: &Project| c.render(p, at, (W, H), &sources).unwrap().data;

    let first = draw(&c, &nested);
    let before = c.nested_stats();
    let again = draw(&c, &nested);
    let after = c.nested_stats();
    assert_eq!(first, again, "the cached frame is the frame");
    assert_eq!(
        after.frame_hits,
        before.frame_hits + 1,
        "the outer compound clip came from the cache: {after:?}"
    );

    // Move the overlay inside the innermost compound clip.
    let white = nested
        .materials
        .sequences
        .iter_mut()
        .flat_map(|s| s.tracks.iter_mut())
        .flat_map(|t| t.segments.iter_mut())
        .find(|s| s.material_id == "white")
        .expect("the overlay is inside");
    white.transform.position = [-0.3, 0.2];
    let moved = draw(&c, &nested);
    assert_ne!(moved, first, "an edit inside shows");
    assert_eq!(
        moved,
        draw(&Compositor::new(Arc::clone(&ctx)), &nested),
        "and matches a compositor that never saw the old frame"
    );

    // Brighten the grade the counter inside names.
    nested.materials.color_adjusts[0].brightness = 0.3;
    let graded = draw(&c, &nested);
    assert_ne!(
        graded, moved,
        "an edit to a pool entry a clip inside names shows"
    );
    assert_eq!(graded, draw(&Compositor::new(ctx), &nested));
}

/// A scratch directory for files a test edits on disk.
fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("compound-files")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Write `text` to `path` and move its modification time on, so a cache keyed
/// by the time sees a new file even inside one clock tick.
fn rewrite(path: &std::path::Path, text: &str, tick: u64) {
    std::fs::write(path, text).unwrap();
    let file = std::fs::File::options().write(true).open(path).unwrap();
    let when = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000 + tick);
    file.set_modified(when).unwrap();
}

#[test]
fn a_lut_file_edited_in_place_refreshes_the_nested_frame() {
    let _ = require_media!();
    let ctx = require_gpu!();
    let Some(mut original) = laid_out() else {
        return;
    };
    let dir = scratch("lut");
    let lut = dir.join("look.cube");
    rewrite(&lut, "LUT_1D_SIZE 2\n0 0 0\n1 1 1\n", 1);
    let mut look = chukcut_engine::modules::project::document::ColorAdjustMaterial::identity();
    look.id = "look".into();
    look.lut = Some(chukcut_engine::modules::project::document::LutRef {
        path: lut.to_string_lossy().into_owned(),
        intensity: 1.0,
    });
    original.materials.color_adjusts.push(look);
    original.tracks[0].segments[0].extras.push("look".into());
    let (nested, _) = nested(&original);
    let at = S + 10;
    let c = Compositor::new(Arc::clone(&ctx));
    let sources = MediaSourceProvider::from_project(&nested);
    let draw = |c: &Compositor| c.render(&nested, at, (W, H), &sources).unwrap().data;

    let identity = draw(&c);
    assert_eq!(identity, draw(&c), "the cached frame is the frame");
    // The same document, the LUT file darkened on disk.
    rewrite(&lut, "LUT_1D_SIZE 2\n0 0 0\n0.25 0.25 0.25\n", 2);
    let darkened = draw(&c);
    assert_ne!(
        darkened, identity,
        "the edited LUT shows inside the compound clip"
    );
    assert_eq!(
        darkened,
        draw(&Compositor::new(Arc::clone(&ctx))),
        "and matches a compositor that never saw the old file"
    );
    // A LUT gone renders unadjusted, and its return shows again.
    std::fs::remove_file(&lut).unwrap();
    let without = draw(&c);
    assert_eq!(
        max_difference(&without, &identity).0,
        0,
        "no LUT is the identity"
    );
    rewrite(&lut, "LUT_1D_SIZE 2\n0 0 0\n0.25 0.25 0.25\n", 3);
    assert_eq!(draw(&c), darkened, "the LUT that came back is drawn");
}

#[test]
fn a_missing_file_that_comes_back_refreshes_the_nested_frame() {
    let media = require_media!();
    let ctx = require_gpu!();
    let Some(mut original) = laid_out() else {
        return;
    };
    // The overlay plays a copy of its file that the test can take away.
    let dir = scratch("missing");
    let copy = dir.join("white.mp4");
    std::fs::copy(&media.solid_white_wide, &copy).unwrap();
    let white = original
        .materials
        .videos
        .iter_mut()
        .find(|m| m.id == "white")
        .unwrap();
    white.path = copy.to_string_lossy().into_owned();
    let (nested, _) = nested(&original);
    let at = S + 10;
    let c = Compositor::new(Arc::clone(&ctx));
    let sources = MediaSourceProvider::from_project(&nested);
    let draw = |c: &Compositor| c.render(&nested, at, (W, H), &sources).unwrap().data;

    let present = draw(&c);
    let before = c.nested_stats();
    assert_eq!(draw(&c), present);
    assert_eq!(c.nested_stats().frame_hits, before.frame_hits + 1);
    // The provider keeps decoded frames of its own, so what is checked here is
    // that the nested frame is not served from the cache once the file has
    // gone, and is again once it is back.
    let hidden = dir.join("white.mp4.away");
    std::fs::rename(&copy, &hidden).unwrap();
    let before = c.nested_stats();
    let _ = draw(&c);
    let after = c.nested_stats();
    assert!(
        after.frame_hits == before.frame_hits && after.frame_misses > before.frame_misses,
        "a file gone from disk renders the inside again: {before:?} → {after:?}"
    );
    std::fs::rename(&hidden, &copy).unwrap();
    let back = draw(&c);
    assert_eq!(
        max_difference(&back, &present).0,
        0,
        "the file that came back is drawn again, not the placeholder"
    );
}

/// The outer compound clip of `nested` at twice the speed: it shows the same
/// 1.5 s of its contents in 0.75 s.
fn at_double_speed(nested: &Project, outer: &str) -> Project {
    let mut p = nested.clone();
    let clip = p.segment_mut(outer).unwrap();
    clip.speed = 2.0;
    clip.target_range.duration = 3 * S / 4;
    let errors: Vec<String> = p
        .validate()
        .into_iter()
        .filter(|i| i.severity == Severity::Error)
        .map(|i| i.message)
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
    p
}

/// The outer compound clip of `nested` on the Hero speed ramp.
fn on_a_curve(nested: &Project, outer: &str) -> Project {
    use chukcut_engine::modules::project::SpeedPreset;
    use chukcut_engine::modules::speed::edit::{set_curve_command, CurveChange};
    let mut p = nested.clone();
    let command = set_curve_command(
        &p,
        outer,
        CurveChange::Preset {
            preset: SpeedPreset::Hero,
        },
    )
    .unwrap();
    apply(&mut p, command);
    p
}

/// Every frame of `a` against `b`, a tenth of a second apart, plus the
/// counter digits where the counter shows.
fn same_frames(c: &Compositor, a: &Project, b: &Project, what: &str) {
    assert_eq!(a.duration(), b.duration(), "{what}: same length");
    let mut at = 10;
    while at < a.duration() {
        let fa = render(c, a, at);
        let fb = render(c, b, at);
        assert_eq!(
            read_counter_rgba(&fa, W, H),
            read_counter_rgba(&fb, W, H),
            "{what}: the same counter frame at {at} µs"
        );
        let (worst, pixel) = max_difference(&fa, &fb);
        assert!(
            worst <= 2,
            "{what}: at {at} µs off by {worst} at pixel ({}, {})",
            pixel as u32 % W,
            pixel as u32 / W
        );
        at += S / 20;
    }
}

fn mixes_agree(a: &Project, b: &Project, tolerance: f32, what: &str) {
    let cancel = AtomicBool::new(false);
    let source = chukcut_engine::modules::audio::FileAudioSource;
    let ma = mix_timeline(a, &source, 48_000, 2, &cancel).unwrap();
    let mb = mix_timeline(b, &source, 48_000, 2, &cancel).unwrap();
    assert_eq!(ma.len(), mb.len(), "{what}");
    let worst = ma
        .iter()
        .zip(&mb)
        .map(|(x, y)| (x - y).abs())
        .fold(0.0f32, f32::max);
    assert!(worst < tolerance, "{what}: the mixes differ by {worst}");
    assert!(ma.iter().any(|s| s.abs() > 0.05), "{what}: there is sound");
}

fn envelopes_agree(a: &Project, b: &Project, what: &str) {
    let cancel = AtomicBool::new(false);
    let source = chukcut_engine::modules::audio::FileAudioSource;
    let ma = mix_timeline(a, &source, 48_000, 2, &cancel).unwrap();
    let mb = mix_timeline(b, &source, 48_000, 2, &cancel).unwrap();
    assert_eq!(ma.len(), mb.len(), "{what}");
    let rms = |x: &[f32]| (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).sqrt();
    let (ra, rb) = (rms(&ma), rms(&mb));
    assert!(ra > 0.01, "{what}: there is sound");
    assert!(
        (ra - rb).abs() <= 0.15 * ra.max(rb),
        "{what}: {ra} against {rb}"
    );
}

/// The sound pieces the mixer gets for `nested`'s compound clips against the
/// clips `flat` holds: the same place, the same part of the file, the same
/// curve.
fn same_sound_pieces(nested: &Project, flat: &Project, material: &str) {
    let heard = chukcut_engine::modules::sequence::audio::flatten_audio(nested);
    type Piece = (TimeRange, TimeRange, Option<Vec<(Micros, f32)>>);
    let pieces = |p: &Project| -> Vec<Piece> {
        p.tracks
            .iter()
            .flat_map(|t| t.segments.iter())
            .filter(|s| s.material_id == material)
            .map(|s| {
                (
                    s.target_range,
                    s.source_range,
                    p.materials
                        .speed_curve_of(s)
                        .map(|c| c.points.iter().map(|q| (q.source, q.speed)).collect()),
                )
            })
            .collect()
    };
    let a = pieces(heard.as_ref());
    let b = pieces(flat);
    assert_eq!(a.len(), b.len());
    for ((ta, sa, ca), (tb, sb, cb)) in a.iter().zip(&b) {
        assert!((ta.start - tb.start).abs() <= 2 && (ta.duration - tb.duration).abs() <= 4);
        assert!((sa.start - sb.start).abs() <= 2 && (sa.duration - sb.duration).abs() <= 4);
        let (ca, cb) = (ca.as_ref().unwrap(), cb.as_ref().unwrap());
        assert_eq!(ca.len(), cb.len());
        for (pa, pb) in ca.iter().zip(cb) {
            assert!(
                (pa.0 - pb.0).abs() <= 2 && (pa.1 - pb.1).abs() < 1e-4,
                "{ca:?} {cb:?}"
            );
        }
    }
}

#[test]
fn a_compound_clip_at_double_speed_flattens_into_the_same_frames_and_sound() {
    let _ = require_media!();
    let ctx = require_gpu!();
    let Some(original) = laid_out() else {
        return;
    };
    let (nested, outer) = nested(&original);
    let fast = at_double_speed(&nested, &outer);
    let flat = flattened(&fast, &outer);
    let c = Compositor::with_config(
        ctx,
        CompositorConfig {
            strict_sources: true,
            ..Default::default()
        },
    );
    same_frames(&c, &fast, &flat, "double speed");
    // Both sides play the sine at twice the speed through the same
    // pitch-preserving render.
    mixes_agree(&fast, &flat, 1e-3, "double speed");
}

#[test]
fn a_compound_clip_on_a_speed_curve_flattens_into_the_same_frames_and_sound() {
    let _ = require_media!();
    let ctx = require_gpu!();
    let Some(original) = laid_out() else {
        return;
    };
    let (nested, outer) = nested(&original);
    let ramped = on_a_curve(&nested, &outer);
    let flat = flattened(&ramped, &outer);
    // Every clip that runs with time took the curve.
    for s in flat.tracks.iter().flat_map(|t| t.segments.iter()) {
        if matches!(s.material_id.as_str(), "counter" | "quadrants" | "sine") {
            assert!(
                flat.materials.speed_curve_of(s).is_some(),
                "{} plays on a curve",
                s.material_id
            );
        }
    }
    let c = Compositor::with_config(
        ctx,
        CompositorConfig {
            strict_sources: true,
            ..Default::default()
        },
    );
    same_frames(&c, &ramped, &flat, "speed curve");
    // A pitch-preserving render through a curve does not come out sample
    // for sample the same twice — the same document mixed twice differs by
    // as much (docs/STATUS.md) — so the sound is compared by what the mixer
    // is handed, and by its level.
    same_sound_pieces(&ramped, &flat, "sine");
    envelopes_agree(&ramped, &flat, "speed curve");
}

#[test]
fn a_compound_clips_volume_keyframes_reach_the_mix() {
    let _ = require_media!();
    let Some(original) = laid_out() else {
        return;
    };
    let (mut nested, outer) = nested(&original);
    // Fade the compound clip from silence to full over its 1.5 s.
    use chukcut_engine::modules::project::document::{AnimatableProperty, Keyframe, KeyframeTrack};
    nested
        .segment_mut(&outer)
        .unwrap()
        .keyframes
        .push(KeyframeTrack {
            property: AnimatableProperty::Volume,
            keyframes: vec![
                Keyframe {
                    time: 0,
                    value: 0.0,
                    easing: Default::default(),
                },
                Keyframe {
                    time: 3 * S / 2,
                    value: 1.0,
                    easing: Default::default(),
                },
            ],
        });
    let cancel = AtomicBool::new(false);
    let source = chukcut_engine::modules::audio::FileAudioSource;
    let peak = |p: &Project, from: Micros, to: Micros| {
        let mix = mix_timeline(p, &source, 48_000, 2, &cancel).unwrap();
        let a = (from * 48 / 1000) as usize * 2;
        let b = (to * 48 / 1000) as usize * 2;
        mix[a..b].iter().fold(0.0f32, |m, s| m.max(s.abs()))
    };
    // The compound clip plays its contents' 0.25..1.75 s at 0.5..2 s: quiet
    // at its start, near full at its end — against the same stretch of the
    // sine without the fade, where the inner clip's own volume applies.
    let early = peak(&nested, S / 2, S / 2 + S / 20);
    let plain_early = peak(&original, S / 4, S / 4 + S / 20);
    let late = peak(&nested, 19 * S / 10, 2 * S);
    let plain_late = peak(&original, 33 * S / 20, 35 * S / 20);
    assert!(
        early < 0.1 * plain_early,
        "faded in: {early} against {plain_early}"
    );
    assert!(
        late > 0.85 * plain_late,
        "and full at the end: {late} against {plain_late}"
    );
}

#[test]
fn loudness_and_silence_hear_a_compound_clips_contents() {
    let _ = require_media!();
    let Some(original) = laid_out() else {
        return;
    };
    let (nested, outer) = nested(&original);
    let state = chukcut_engine::state::AppState::new();
    *state.project.write() = Some(nested.clone());
    let cancel = AtomicBool::new(false);

    let clip = chukcut_engine::modules::loudness::commands::loudness_measure_clip(
        &state,
        outer.clone(),
        &cancel,
    )
    .expect("a compound clip has a loudness");
    assert!(clip.loudness.integrated.is_some(), "{clip:?}");

    let analysis = chukcut_engine::modules::silence::commands::silence_analyse(
        &state,
        outer.clone(),
        false,
        &cancel,
    )
    .expect("a compound clip can be analysed");
    let window = nested.segment(&outer).unwrap().1.source_range;
    assert_eq!(analysis.source, window);
    assert!(
        analysis.envelope.peak_db(window) > -40.0,
        "the sine inside is heard"
    );

    // The mix is the root timeline's, also from inside the compound clip.
    let whole =
        chukcut_engine::modules::loudness::commands::loudness_measure_mix(&state, &cancel).unwrap();
    let open = build::open(&nested, &outer).unwrap();
    if let Some(p) = state.project.write().as_mut() {
        open.apply(p).unwrap();
    }
    let inside =
        chukcut_engine::modules::loudness::commands::loudness_measure_mix(&state, &cancel).unwrap();
    assert_eq!(whole.integrated, inside.integrated);
}
