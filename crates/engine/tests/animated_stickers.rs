//! Animated stickers end to end: a Lottie and a GIF as image clips, through
//! the compositor (looping and played once) and through an export, frame by
//! frame.

mod support;

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use chukcut_engine::modules::animated::playback::{set_playback_command, Playback};
use chukcut_engine::modules::export::{
    job, resolve_settings, run_export, ExportJob, ExportRequest, FnSink,
};
use chukcut_engine::modules::media::{MediaSourceProvider, VideoDecoder};
use chukcut_engine::modules::project::document::{
    CanvasConfig, ImageMaterial, Micros, Project, Track, TrackKind,
};
use chukcut_engine::modules::render::{Compositor, CompositorConfig};
use chukcut_engine::modules::timeline::History;

use support::{pixel, segment};

/// 100×50, 2 s at 30 fps: a half-transparent red 50×50 box sliding from the
/// left half (centre x 25) to the right half (centre x 75).
const SLIDE: &str = r#"{
  "v":"5.7.0","fr":30,"ip":0,"op":60,"w":100,"h":50,"nm":"slide","ddd":0,"assets":[],
  "layers":[{"ddd":0,"ind":1,"ty":4,"nm":"box","sr":1,
    "ks":{"o":{"a":0,"k":100},"r":{"a":0,"k":0},
          "p":{"a":1,"k":[{"t":0,"s":[25,25,0],"i":{"x":[1],"y":[1]},"o":{"x":[0],"y":[0]}},{"t":60,"s":[75,25,0]}]},
          "a":{"a":0,"k":[0,0,0]},"s":{"a":0,"k":[100,100,100]}},
    "ao":0,"ip":0,"op":60,"st":0,"bm":0,
    "shapes":[{"ty":"gr","nm":"g","it":[
      {"ty":"rc","nm":"r","d":1,"s":{"a":0,"k":[50,50]},"p":{"a":0,"k":[0,0]},"r":{"a":0,"k":0}},
      {"ty":"fl","nm":"f","c":{"a":0,"k":[1,0,0,1]},"o":{"a":0,"k":100},"r":1},
      {"ty":"tr","p":{"a":0,"k":[0,0]},"a":{"a":0,"k":[0,0]},"s":{"a":0,"k":[100,100]},"r":{"a":0,"k":0},"o":{"a":0,"k":100}}
    ]}]}]
}"#;

fn dir() -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("animated");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The 200×100 canvas with one sticker clip of `file` covering it for 5 s.
fn sticker_project(file: &Path, playback: Playback) -> (Project, String) {
    let mut project = Project::new(
        "stickers",
        CanvasConfig {
            width: 200,
            height: 100,
            background: [0.0, 0.0, 0.0, 1.0],
        },
        30.0,
    );
    let animation = chukcut_engine::modules::animated::load(file).expect("an animation");
    let (width, height) = animation.size();
    project.materials.images.push(ImageMaterial {
        id: "sticker".into(),
        path: file.to_string_lossy().into_owned(),
        width,
        height,
    });
    let mut track = Track::new(TrackKind::Video, "V1");
    let clip = segment("sticker", 0, 5_000_000);
    let id = clip.id.clone();
    track.segments.push(clip);
    project.tracks.push(track);
    if playback != Playback::Loop {
        let (entry, command) = set_playback_command(&project, &id, playback).expect("edit");
        let (key, value) = entry.expect("a block");
        project.materials.extras.insert(key, value);
        History::new().apply(&mut project, command).expect("apply");
    }
    (project, id)
}

/// Each test writes its own copy: the tests run in parallel, and a shared
/// file was truncated by one test's write while the other was reading it.
fn lottie_file(name: &str) -> PathBuf {
    let path = dir().join(format!("{name}.json"));
    std::fs::write(&path, SLIDE).unwrap();
    path
}

/// Where the box's centre is on the 200-px canvas at `time` into the clip,
/// looping.
fn box_centre(time: Micros) -> f64 {
    let t = (time.rem_euclid(2_000_000)) as f64 / 2_000_000.0;
    (25.0 + 50.0 * t) * 2.0
}

fn red(data: &[u8], x: u32) -> u8 {
    pixel(data, 200, x, 50)[0]
}

#[test]
fn a_lottie_sticker_loops_or_holds_its_last_frame() {
    let ctx = require_gpu!();
    let file = lottie_file("slide-playback");
    let compositor = Compositor::new(ctx);
    let (looping, _) = sticker_project(&file, Playback::Loop);
    let (once, _) = sticker_project(&file, Playback::Once);
    let render = |project: &Project, time: Micros| {
        compositor
            .render(
                project,
                time,
                (200, 100),
                &MediaSourceProvider::from_project(project),
            )
            .expect("render")
            .data
    };

    let early = render(&looping, 500_000);
    let centre = box_centre(500_000) as u32;
    assert!(red(&early, centre) > 200, "the box is at {centre}");
    assert!(red(&early, 190) < 10, "and not at the right edge");
    // Looping: 2.5 s is 0.5 s into the second pass, the same picture.
    assert_eq!(render(&looping, 2_500_000), early);

    // Played once: the box stays where the animation ends.
    let held = render(&once, 2_500_000);
    assert!(red(&held, 150) > 200);
    assert!(red(&held, 30) < 10);
    assert_eq!(render(&once, 500_000), early, "the same until it ends");
}

#[test]
fn a_gif_sticker_shows_each_frame_for_its_own_delay() {
    let ctx = require_gpu!();
    let path = dir().join("three.gif");
    {
        use image::codecs::gif::GifEncoder;
        use image::{Delay, Frame, Rgba, RgbaImage};
        let mut encoder = GifEncoder::new(std::fs::File::create(&path).unwrap());
        for (colour, ms) in [
            ([255, 0, 0, 255], 50),
            ([0, 255, 0, 255], 100),
            ([0, 0, 255, 255], 100),
        ] {
            encoder
                .encode_frame(Frame::from_parts(
                    RgbaImage::from_pixel(20, 10, Rgba(colour)),
                    0,
                    0,
                    Delay::from_numer_denom_ms(ms, 1),
                ))
                .unwrap();
        }
    }
    let (project, _) = sticker_project(&path, Playback::Loop);
    let compositor = Compositor::new(ctx);
    let sources = MediaSourceProvider::from_project(&project);
    let colour = |time: Micros| {
        let frame = compositor
            .render(&project, time, (200, 100), &sources)
            .expect("render");
        let [r, g, b, _] = pixel(&frame.data, 200, 100, 50);
        [r, g, b]
    };
    assert_eq!(colour(20_000), [255, 0, 0]);
    assert_eq!(colour(60_000), [0, 255, 0]);
    assert_eq!(colour(170_000), [0, 0, 255]);
    // 250 ms a pass: 270 ms is 20 ms into the second.
    assert_eq!(colour(270_000), [255, 0, 0]);
    // And back again, out of order, through the cache.
    assert_eq!(colour(60_000), [0, 255, 0]);
}

#[test]
fn an_export_draws_every_frame_of_the_animation_where_it_belongs() {
    let ctx = require_gpu!();
    let file = lottie_file("slide-export");
    let (project, _) = sticker_project(&file, Playback::Loop);
    let path = dir().join("stickers.mp4");
    let _ = std::fs::remove_file(&path);
    let request = ExportRequest {
        output_path: path.to_string_lossy().into_owned(),
        preset_id: None,
        overrides: None,
        hardware: None,
        include_audio: false,
        range: Some((0, 3_000_000)),
    };
    let settings = resolve_settings(&project, &request).expect("settings");
    let job = ExportJob {
        job_id: "stickers".into(),
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
    let frame_time = |n: i64| (n as f64 * 1_000_000.0 / 30.0).round() as Micros;
    for n in [0i64, 7, 29, 45, 59, 60, 61, 75, 89] {
        let frame = decoder
            .seek_and_decode(frame_time(n) + 16_000)
            .expect("decode");
        assert_eq!((frame.width, frame.height), (200, 100));
        let centre = box_centre(frame_time(n)).round() as u32;
        // The box is 100 px wide on the canvas: its centre is red, a point
        // 70 px off it (outside, inside the canvas) is not.
        assert!(
            pixel(&frame.data, 200, centre, 50)[0] > 150,
            "frame {n}: the box should be centred at {centre}"
        );
        let away = if centre > 100 {
            centre - 70
        } else {
            centre + 70
        };
        assert!(
            pixel(&frame.data, 200, away, 50)[0] < 60,
            "frame {n}: nothing at {away}"
        );
    }
    let _ = std::fs::remove_file(&path);
}
