//! Keyframe-free motion, in pixels.
//!
//! The curves are unit-tested by number in `modules/motion`; these render
//! real frames through the compositor and check that each kind of animation
//! reaches the picture the way its numbers say: a fade blends, a wipe cuts at
//! the edge, a slide moves the clip, a zoom keeps its pivot still, a blur
//! spreads the clip's edge, and a text animator draws less of its title while
//! it runs than when it rests.

use chukcut_engine::modules::media::MediaSourceProvider;
use chukcut_engine::modules::motion::edit;
use chukcut_engine::modules::project::animation::{
    AnimationPreset as P, AnimationSlot, ClipAnimation, Ease, PunchZoom, TextSlot,
};
use chukcut_engine::modules::project::document::{
    CanvasConfig, Micros, Project, Segment, TextAlign, TextMaterial, TimeRange, Track, TrackKind,
    Transform, VideoMaterial,
};
use chukcut_engine::modules::render::source::{SolidColorProvider, SolidSource};
use chukcut_engine::modules::render::{Compositor, Frame, SourceProvider};

const W: u32 = 200;
const H: u32 = 100;
const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];

fn compositor() -> Option<Compositor> {
    chukcut_engine::modules::gpu::render_context().map(Compositor::new)
}

/// A black canvas with one red full-frame clip on `[0, 4 s)`.
fn project() -> (Project, String) {
    let mut project = Project::new(
        "motion",
        CanvasConfig {
            width: W,
            height: H,
            background: [0.0, 0.0, 0.0, 1.0],
        },
        30.0,
    );
    project.materials.videos.push(VideoMaterial {
        id: "red".into(),
        path: "/nonexistent/red.mp4".into(),
        width: W,
        height: H,
        duration: 4_000_000,
        fps: 30.0,
        has_audio: false,
        rotation: 0,
    });
    let segment = Segment {
        id: "clip".into(),
        material_id: "red".into(),
        target_range: TimeRange::new(0, 4_000_000),
        source_range: TimeRange::new(0, 4_000_000),
        render_index: 0,
        speed: 1.0,
        volume: 1.0,
        transform: Transform::default(),
        crop: None,
        extras: Vec::new(),
        keyframes: Vec::new(),
    };
    let mut track = Track::new(TrackKind::Video, "V1");
    track.segments.push(segment);
    project.tracks.push(track);
    (project, "clip".into())
}

fn sources() -> SolidColorProvider {
    SolidColorProvider::new().with_fallback(SolidSource::new(RED, W, H))
}

fn animate(project: &mut Project, slot: AnimationSlot, preset: P, duration: Micros) {
    let command = edit::set_slot_command(
        project,
        "clip",
        slot,
        Some(ClipAnimation {
            preset,
            duration,
            easing: Ease::Linear,
            strength: 1.0,
        }),
    )
    .unwrap();
    command.apply(project).unwrap();
}

fn frame(c: &Compositor, project: &Project, time: Micros, sources: &dyn SourceProvider) -> Frame {
    c.render(project, time, (W, H), sources).unwrap()
}

fn red(frame: &Frame, x: u32, y: u32) -> u8 {
    frame.pixel(x, y)[0]
}

#[test]
fn a_fade_in_blends_from_the_background_and_a_fade_out_back_to_it() {
    let Some(c) = compositor() else { return };
    let (mut p, _) = project();
    animate(&mut p, AnimationSlot::In, P::Fade, 1_000_000);
    animate(&mut p, AnimationSlot::Out, P::Fade, 1_000_000);
    let s = sources();
    assert!(red(&frame(&c, &p, 0, &s), 100, 50) < 5);
    let mid = red(&frame(&c, &p, 500_000, &s), 100, 50);
    // Half opacity over black, blended in linear light and encoded to sRGB.
    assert!((150..=210).contains(&mid), "{mid}");
    assert!(red(&frame(&c, &p, 2_000_000, &s), 100, 50) > 250);
    let out = red(&frame(&c, &p, 3_500_000, &s), 100, 50);
    assert!((150..=210).contains(&out), "{out}");
}

#[test]
fn a_wipe_shows_the_clip_up_to_its_edge_and_nothing_past_it() {
    let Some(c) = compositor() else { return };
    let (mut p, _) = project();
    animate(&mut p, AnimationSlot::In, P::WipeRight, 1_000_000);
    let f = frame(&c, &p, 250_000, &sources());
    assert!(red(&f, 20, 50) > 250, "left of the edge");
    assert!(red(&f, 80, 50) < 5, "right of the edge");
    let f = frame(&c, &p, 1_500_000, &sources());
    assert!(red(&f, 190, 50) > 250, "after the wipe");
}

#[test]
fn a_slide_moves_the_clip_and_lands_where_it_rests() {
    let Some(c) = compositor() else { return };
    let (mut p, _) = project();
    animate(&mut p, AnimationSlot::In, P::SlideRight, 1_000_000);
    // Slide right starts left of the rest position by 0.6 half-canvases,
    // i.e. 60 px on a 200 px canvas, at half opacity... at t = 0 the opacity
    // is 0, so look a little in: at 25 % the clip is 45 px left.
    let f = frame(&c, &p, 250_000, &sources());
    assert!(red(&f, 199, 50) < 5, "the right edge is uncovered");
    assert!(red(&f, 100, 50) > 100, "the middle is covered");
    let f = frame(&c, &p, 2_000_000, &sources());
    assert!(red(&f, 199, 50) > 250);
}

#[test]
fn a_punch_in_zoom_keeps_its_pivot_still() {
    let Some(c) = compositor() else { return };
    let (mut p, _) = project();
    // A half-size clip in the middle, punched in 2x about its own left edge:
    // the left edge stays at x = 50 and the right edge goes to the canvas edge
    // and beyond.
    p.tracks[0].segments[0].transform.scale = [0.5, 0.5];
    edit::set_zoom_command(
        &p,
        "clip",
        Some(PunchZoom {
            amount: 2.0,
            pivot: [-0.5, 0.0],
            duration: 0,
            easing: Ease::Linear,
        }),
    )
    .unwrap()
    .apply(&mut p)
    .unwrap();
    let f = frame(&c, &p, 1_000_000, &sources());
    assert!(red(&f, 45, 50) < 5, "left of the pivot stays empty");
    assert!(red(&f, 55, 50) > 250);
    assert!(
        red(&f, 195, 50) > 250,
        "the clip now reaches the right edge"
    );
    assert!(red(&f, 100, 5) > 250, "and the top");
}

#[test]
fn a_blur_in_softens_the_edge_and_then_sharpens() {
    let Some(c) = compositor() else { return };
    let (mut p, _) = project();
    p.tracks[0].segments[0].transform.scale = [0.5, 0.5];
    animate(&mut p, AnimationSlot::In, P::Blur, 1_000_000);
    let s = sources();
    let sharp = frame(&c, &p, 2_000_000, &s);
    let soft = frame(&c, &p, 400_000, &s);
    // Just outside the clip's left edge (x = 50): nothing when sharp, some
    // spill while blurred.
    assert!(red(&sharp, 46, 50) < 5);
    assert!(red(&soft, 46, 50) > 10, "{}", red(&soft, 46, 50));
    assert!(red(&sharp, 100, 50) > 250);
}

#[test]
fn a_typewriter_draws_less_of_the_title_while_it_types() {
    let Some(c) = compositor() else { return };
    let mut p = Project::new(
        "text",
        CanvasConfig {
            width: 640,
            height: 360,
            background: [0.0, 0.0, 0.0, 1.0],
        },
        30.0,
    );
    p.materials.texts.push(TextMaterial {
        id: "title".into(),
        content: "TYPEWRITER".into(),
        font_family: "sans-serif".into(),
        font_size: 64.0,
        color: [1.0, 1.0, 1.0, 1.0],
        bold: true,
        italic: false,
        align: TextAlign::Center,
        stroke_width: 0.0,
        stroke_color: [0.0, 0.0, 0.0, 1.0],
        shadow: None,
        background: None,
        caption: None,
        ..Default::default()
    });
    let mut track = Track::new(TrackKind::Text, "T1");
    track.segments.push(Segment {
        id: "clip".into(),
        material_id: "title".into(),
        target_range: TimeRange::new(0, 4_000_000),
        source_range: TimeRange::new(0, 4_000_000),
        render_index: 0,
        speed: 1.0,
        volume: 1.0,
        transform: Transform::default(),
        crop: None,
        extras: Vec::new(),
        keyframes: Vec::new(),
    });
    p.tracks.push(track);
    let typewriter = chukcut_engine::modules::motion::catalog::text_preset(
        chukcut_engine::modules::project::animation::TextPreset::Typewriter,
    )
    .animator;
    edit::set_text_command(&p, "clip", TextSlot::In, Some(typewriter))
        .unwrap()
        .apply(&mut p)
        .unwrap();

    let sources = MediaSourceProvider::from_project(&p);
    let ink = |time: Micros| -> u64 {
        let f = c.render(&p, time, (640, 360), &sources).unwrap();
        f.data.chunks_exact(4).map(|px| px[0] as u64).sum()
    };
    let resting = ink(3_000_000);
    if resting == 0 {
        return; // no fonts on this machine
    }
    let typing = ink(400_000);
    let almost = ink(1_100_000);
    assert!(typing < resting / 2, "{typing} of {resting}");
    assert!(
        typing < almost && almost <= resting,
        "{typing} {almost} {resting}"
    );
    assert_eq!(
        ink(1_250_000),
        resting,
        "a finished animator draws the cached title"
    );
}
