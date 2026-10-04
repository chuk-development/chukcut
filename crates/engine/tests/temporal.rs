//! Frame blending and motion blur, end to end: the compositor's averaged
//! draws, the provider's two-frame cache under sequential playback, and an
//! export that carries both.
//!
//! The counter fixture's stripes are black or white per bit of the frame
//! index, so a blend of two frames shows exactly where it is a blend: the
//! stripes that differ between the two frames turn grey by the blend weight,
//! the others stay clean.

mod support;

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use chukcut_engine::modules::export::{
    job, resolve_settings, run_export, ExportJob, ExportRequest, FnSink,
};
use chukcut_engine::modules::fx::catalog::MOTION_BLUR;
use chukcut_engine::modules::media::{MediaSourceProvider, VideoDecoder};
use chukcut_engine::modules::project::document::{
    AnimatableProperty, CanvasConfig, Easing, Keyframe, KeyframeTrack, Micros, Project, TimeRange,
    Track, TrackKind,
};
use chukcut_engine::modules::project::{EffectMaterial, EffectValue};
use chukcut_engine::modules::render::{Compositor, CompositorConfig, SourceProvider};
use chukcut_engine::modules::speed::blend::{set_frame_blend_command, FrameBlend};
use chukcut_engine::modules::timeline::History;

use support::{material_for, pixel, read_counter_rgba, segment, COUNTER_STRIPES, COUNTER_WIDTH};

const FRAME: f64 = 1_000_000.0 / 30.0;

fn canvas() -> CanvasConfig {
    CanvasConfig {
        width: 320,
        height: 240,
        background: [0.0, 0.0, 0.0, 1.0],
    }
}

/// The first second of the counter at 0.25x, four seconds on the timeline,
/// frame blending on when `blend`.
fn slow_counter(blend: bool) -> Option<(Project, String)> {
    let media = support::media().ok()?;
    let mut project = Project::new("blend", canvas(), 30.0);
    project
        .materials
        .videos
        .push(material_for("counter", &media.counter).ok()?);
    let mut track = Track::new(TrackKind::Video, "V1");
    let mut clip = segment("counter", 0, 4_000_000);
    clip.source_range = TimeRange::new(0, 1_000_000);
    clip.speed = 0.25;
    let id = clip.id.clone();
    track.segments.push(clip);
    project.tracks.push(track);
    if blend {
        let mut history = History::new();
        let (entry, command) =
            set_frame_blend_command(&project, &id, FrameBlend::Blend).expect("blend edit");
        let (key, value) = entry.expect("an extras block");
        project.materials.extras.insert(key, value);
        history.apply(&mut project, command).expect("apply");
    }
    Some((project, id))
}

fn to_linear(v: u8) -> f64 {
    let c = v as f64 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn to_srgb(l: f64) -> f64 {
    let c = if l <= 0.003_130_8 {
        l * 12.92
    } else {
        1.055 * l.powf(1.0 / 2.4) - 0.055
    };
    c * 255.0
}

/// The red channel of every stripe's centre on the middle row.
fn stripes(data: &[u8]) -> Vec<u8> {
    let stripe = COUNTER_WIDTH / COUNTER_STRIPES;
    (0..COUNTER_STRIPES)
        .map(|bit| pixel(data, 320, bit * stripe + stripe / 2, 120)[0])
        .collect()
}

#[test]
fn a_blended_frame_mixes_its_two_neighbours_by_where_it_falls() {
    let _ = require_media!();
    let ctx = require_gpu!();
    let Some((plain, _)) = slow_counter(false) else {
        return;
    };
    let Some((blended, _)) = slow_counter(true) else {
        return;
    };
    let compositor = Compositor::new(ctx);
    let plain_sources = MediaSourceProvider::from_project(&plain);
    let blend_sources = MediaSourceProvider::from_project(&blended);
    // Source frame 6 → 7 (0b110 → 0b111: bit 0 differs), at quarter steps:
    // timeline frames 24..28 at 0.25x.
    let render = |project: &Project, sources: &dyn SourceProvider, time: Micros| {
        compositor
            .render(project, time, (320, 240), sources)
            .expect("render")
    };
    let at = |n: i64| (n as f64 * FRAME).round() as Micros + 10;
    let a = render(&plain, &plain_sources, at(24));
    let b = render(&plain, &plain_sources, at(28));
    assert_eq!(read_counter_rgba(&a.data, 320, 240), Some(6));
    assert_eq!(read_counter_rgba(&b.data, 320, 240), Some(7));
    let (sa, sb) = (stripes(&a.data), stripes(&b.data));

    for (n, weight) in [(25, 0.25), (26, 0.5), (27, 0.75)] {
        // Without blending the frame is held: still a clean frame 6.
        let held = render(&plain, &plain_sources, at(n));
        assert_eq!(
            read_counter_rgba(&held.data, 320, 240),
            Some(6),
            "held at {n}"
        );

        let mixed = render(&blended, &blend_sources, at(n));
        let got = stripes(&mixed.data);
        for bit in 0..COUNTER_STRIPES as usize {
            // The compositor's target is sRGB, so the mix is in linear light.
            let want = to_srgb(to_linear(sa[bit]) * (1.0 - weight) + to_linear(sb[bit]) * weight);
            assert!(
                (got[bit] as f64 - want).abs() <= 4.0,
                "frame {n}, stripe {bit}: got {}, want {want:.1} ({} → {})",
                got[bit],
                sa[bit],
                sb[bit]
            );
        }
    }

    // On a source frame there is nothing to mix: byte for byte the plain one.
    let on_frame = render(&blended, &blend_sources, at(24));
    assert_eq!(on_frame.data, a.data);
}

#[test]
fn sequential_blended_playback_decodes_every_frame_in_order() {
    let _ = require_media!();
    let ctx = require_gpu!();
    let Some((project, _)) = slow_counter(true) else {
        return;
    };
    let compositor = Compositor::with_config(
        ctx,
        CompositorConfig {
            strict_sources: true,
            ..Default::default()
        },
    );
    let sources = MediaSourceProvider::from_project(&project);
    // Every frame of the 4 s clip, as the player and the export ask for them:
    // the frames that land on a source frame are that frame, clean, which
    // they would not be if the cache had handed back a neighbour.
    for n in 0..119 {
        let time = (n as f64 * FRAME).round() as Micros + 10;
        let frame = compositor
            .render(&project, time, (320, 240), &sources)
            .expect("render");
        if n % 4 == 0 {
            let want = (n / 4) as u64;
            assert_eq!(
                read_counter_rgba(&frame.data, 320, 240),
                Some(want),
                "timeline frame {n} is source frame {want}"
            );
        }
    }
}

#[test]
fn an_export_carries_the_blend() {
    let _ = require_media!();
    let ctx = require_gpu!();
    let Some((project, _)) = slow_counter(true) else {
        return;
    };
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("temporal");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("blend.mp4");
    let _ = std::fs::remove_file(&path);
    let request = ExportRequest {
        output_path: path.to_string_lossy().into_owned(),
        preset_id: None,
        overrides: None,
        hardware: None,
        include_audio: false,
        range: Some((0, 1_000_000)),
    };
    let settings = resolve_settings(&project, &request).expect("settings");
    let job = ExportJob {
        job_id: "blend".into(),
        project: project.clone(),
        settings,
        compositor: Arc::new(Compositor::with_config(
            ctx,
            CompositorConfig {
                strict_sources: true,
                ..Default::default()
            },
        )),
        sources: Arc::new(MediaSourceProvider::from_project(&project)),
        audio: job::audio_source(),
        cancel: Arc::new(AtomicBool::new(false)),
    };
    run_export(&job, &FnSink(|_| {})).expect("export");

    let mut decoder = VideoDecoder::open(&path).expect("open the export");
    // Frame 26 is half of source frame 6 and half of 7: the stripe of bit 0
    // (0 in 6, 1 in 7) is grey, the shared stripes stay clean.
    let frame = decoder
        .seek_and_decode((26.0 * FRAME).round() as Micros + 16_000)
        .expect("decode");
    let s = stripes(&frame.data);
    assert!((100..=230).contains(&s[0]), "bit 0 is mixed: {s:?}");
    assert!(
        s[1] > 200 && s[2] > 200,
        "bits 1 and 2 are set in both: {s:?}"
    );
    assert!(s[3] < 40, "bit 3 is clear in both: {s:?}");
    let _ = std::fs::remove_file(&path);
}

/// A white box (the wide solid at a quarter scale) crossing the 320×240
/// canvas left to right in one second, with motion blur at `shutter`
/// degrees, or none.
fn moving_box(shutter: Option<f32>) -> Option<Project> {
    let media = support::media().ok()?;
    let mut project = Project::new("blur", canvas(), 30.0);
    project
        .materials
        .videos
        .push(material_for("white", &media.solid_white_wide).ok()?);
    let mut track = Track::new(TrackKind::Video, "V1");
    let mut clip = segment("white", 0, 1_000_000);
    clip.transform.scale = [0.25, 0.25];
    clip.keyframes.push(KeyframeTrack {
        property: AnimatableProperty::PositionX,
        keyframes: vec![
            Keyframe {
                time: 0,
                value: -1.0,
                easing: Easing::Linear,
            },
            Keyframe {
                time: 1_000_000,
                value: 1.0,
                easing: Easing::Linear,
            },
        ],
    });
    if let Some(shutter) = shutter {
        let mut effect = EffectMaterial::new(MOTION_BLUR);
        effect
            .params
            .insert("shutter".into(), EffectValue::Number(shutter));
        clip.extras.push(effect.id.clone());
        project.materials.effects.push(effect);
    }
    track.segments.push(clip);
    project.tracks.push(track);
    Some(project)
}

/// Pixels of the middle row that are neither background nor box.
fn partial(data: &[u8]) -> usize {
    (0..320)
        .map(|x| pixel(data, 320, x, 120)[0])
        .filter(|&v| v > 12 && v < 243)
        .count()
}

fn row_light(data: &[u8]) -> f64 {
    (0..320)
        .map(|x| to_linear(pixel(data, 320, x, 120)[0]))
        .sum()
}

#[test]
fn motion_blur_smears_a_fast_move_and_keeps_its_light() {
    let _ = require_media!();
    let ctx = require_gpu!();
    let (Some(sharp), Some(blurred)) = (moving_box(None), moving_box(Some(180.0))) else {
        return;
    };
    let compositor = Compositor::new(ctx);
    let time = 500_000;
    let sharp_frame = compositor
        .render(
            &sharp,
            time,
            (320, 240),
            &MediaSourceProvider::from_project(&sharp),
        )
        .expect("render");
    let blurred_frame = compositor
        .render(
            &blurred,
            time,
            (320, 240),
            &MediaSourceProvider::from_project(&blurred),
        )
        .expect("render");
    // 320 px a second at 30 fps, half the frame: a smear of about 5 px on
    // each vertical edge.
    assert!(
        partial(&sharp_frame.data) <= 2,
        "the sharp box has hard edges"
    );
    let smeared = partial(&blurred_frame.data);
    assert!((6..=14).contains(&smeared), "{smeared} smeared pixels");
    // Averaging moves light, it does not make or lose it.
    let (a, b) = (row_light(&sharp_frame.data), row_light(&blurred_frame.data));
    assert!((a - b).abs() / a < 0.03, "row light {a} vs {b}");
    // Centred shutter: the smear is symmetric about the sharp box.
    let lit = |data: &[u8]| -> f64 {
        let (mut sum, mut weight) = (0.0, 0.0);
        for x in 0..320 {
            let l = to_linear(pixel(data, 320, x, 120)[0]);
            sum += l * x as f64;
            weight += l;
        }
        sum / weight
    };
    assert!((lit(&sharp_frame.data) - lit(&blurred_frame.data)).abs() < 0.5);
}

#[test]
fn a_still_clip_with_motion_blur_is_drawn_exactly_once() {
    let _ = require_media!();
    let ctx = require_gpu!();
    let (Some(mut sharp), Some(mut blurred)) = (moving_box(None), moving_box(Some(360.0))) else {
        return;
    };
    for project in [&mut sharp, &mut blurred] {
        project.tracks[0].segments[0].keyframes.clear();
    }
    let compositor = Compositor::new(ctx);
    let render = |p: &Project| {
        compositor
            .render(
                p,
                400_000,
                (320, 240),
                &MediaSourceProvider::from_project(p),
            )
            .expect("render")
            .data
    };
    assert_eq!(render(&sharp), render(&blurred));
}
