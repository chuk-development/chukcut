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
