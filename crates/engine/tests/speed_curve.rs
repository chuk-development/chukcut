//! Speed curves end to end: the edit layer (split, trim, undo, links), the
//! compositor reading the right frames, and an export whose length and
//! content follow the curve.
//!
//! The counter fixture writes its own frame index into its pixels, so "which
//! frame of the source is on screen" is read back from the rendered or
//! exported picture rather than trusted.

mod support;

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use chukcut_engine::modules::export::{
    job, resolve_settings, run_export, ExportJob, ExportRequest, FnSink,
};
use chukcut_engine::modules::media::{MediaSourceProvider, VideoDecoder};
use chukcut_engine::modules::project::document::{
    CanvasConfig, Micros, Project, Severity, TimeRange, Track, TrackKind,
};
use chukcut_engine::modules::project::speed::{curve_target_duration, SpeedPreset};
use chukcut_engine::modules::render::{Compositor, CompositorConfig, SourceProvider};
use chukcut_engine::modules::speed::edit::{set_curve_command, CurveChange};
use chukcut_engine::modules::timeline::ops::{split_at, EditCommand};
use chukcut_engine::modules::timeline::History;

use support::{canonical, material_for, probe_output, read_counter_rgba, segment};

const FRAME: f64 = 1_000_000.0 / 30.0;

/// One 4 s clip of an (imaginary) 8 s file starting 1 s in, on a 30 fps
/// timeline, with a 1 s clip after it.
fn two_clips() -> (Project, String, String) {
    let mut project = Project::new("speed", CanvasConfig::default(), 30.0);
    let mut track = Track::new(TrackKind::Video, "V1");
    let mut a = segment("m", 0, 4_000_000);
    a.source_range = TimeRange::new(1_000_000, 4_000_000);
    let b = segment("n", 4_000_000, 1_000_000);
    let (a_id, b_id) = (a.id.clone(), b.id.clone());
    track.segments.push(a);
    track.segments.push(b);
    project.tracks.push(track);
    (project, a_id, b_id)
}

fn ramp(project: &mut Project, history: &mut History, id: &str, preset: SpeedPreset) {
    let command = set_curve_command(project, id, CurveChange::Preset { preset }).unwrap();
    history.apply(project, command).unwrap();
}

fn no_errors(project: &Project) {
    let errors: Vec<_> = project
        .validate()
        .into_iter()
        .filter(|i| i.severity == Severity::Error)
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
}

/// The source instant on screen at timeline `time`, through whichever clip
/// of the lane covers it.
fn source_on_screen(project: &Project, time: Micros) -> Option<Micros> {
    let segment = project.tracks[0].segment_at(time)?;
    project.materials.time_map(segment).source_time_at(time)
}

#[test]
fn a_split_ramp_plays_exactly_like_the_whole_one() {
    let (mut project, a, _) = two_clips();
    let mut history = History::default();
    ramp(&mut project, &mut history, &a, SpeedPreset::Hero);
    let whole = project.clone();
    let length = whole.segment(&a).unwrap().1.target_range.duration;

    // Cut in the middle of the slow-motion, where the speed changes fastest.
    let at = length / 2 + 12_345;
    let before_split = canonical(&project);
    let split = split_at(&project, &a, at).unwrap();
    history.apply(&mut project, split).unwrap();
    no_errors(&project);

    let lane = &project.tracks[0].segments;
    let (left, right) = (&lane[0], &lane[1]);
    assert_eq!(left.target_range.end(), right.target_range.start);
    assert_eq!(
        left.source_range.end(),
        right.source_range.start,
        "the halves read the material without a gap or an overlap"
    );
    let curve = |s| project.materials.speed_curve_of(s).map(|c| c.id.clone());
    assert_eq!(curve(left), curve(right), "both halves keep the one curve");
    assert!(curve(left).is_some());

    // Every frame of the timeline shows the same source instant as before,
    // to within the rounding of one microsecond of source at 0.25x.
    let mut t = 0;
    while t < length {
        let a = source_on_screen(&whole, t).unwrap();
        let b = source_on_screen(&project, t).unwrap();
        assert!((a - b).abs() <= 6, "at {t}: {a} unsplit, {b} split");
        t += 8_333;
    }
    // And the speed is continuous across the cut.
    let speed = |s, offset| project.materials.time_map(s).speed_at(offset);
    let before = speed(left, left.target_range.duration - 1);
    let after = speed(right, 0);
    assert!((before - after).abs() < 1e-3, "{before} vs {after}");

    history.undo(&mut project).unwrap();
    assert_eq!(canonical(&project), before_split, "undo is exact");
}

#[test]
fn a_trimmed_ramp_keeps_the_slow_motion_where_it_was_in_the_footage() {
    let (mut project, a, b) = two_clips();
    let mut history = History::default();
    ramp(&mut project, &mut history, &a, SpeedPreset::FlashOut);
    let whole = project.clone();
    let (_, seg) = project.segment(&a).unwrap();
    let seg = seg.clone();

    // Trim the head by 0.3 s of timeline, as the timeline's trim does.
    let new_target = TimeRange::new(300_000, seg.target_range.duration - 300_000);
    let source = project.materials.time_map(&seg).retimed_source(new_target);
    let trim = EditCommand::TrimSegment {
        segment_id: a.clone(),
        before_target: seg.target_range,
        before_source: seg.source_range,
        after_target: new_target,
        after_source: source,
    };
    history.apply(&mut project, trim).unwrap();
    no_errors(&project);
    for t in (300_000..new_target.end()).step_by(10_007) {
        let x = source_on_screen(&whole, t).unwrap();
        let y = source_on_screen(&project, t).unwrap();
        assert!((x - y).abs() <= 2, "at {t}: {x} vs {y}");
    }
    assert_eq!(
        project.segment(&b).unwrap().1.target_range.start,
        seg.target_range.end()
    );

    // A trim the curve disagrees with is refused.
    let (_, now) = project.segment(&a).unwrap();
    let wrong = EditCommand::TrimSegment {
        segment_id: a.clone(),
        before_target: now.target_range,
        before_source: now.source_range,
        after_target: TimeRange::new(now.target_range.start, now.target_range.duration - 100_000),
        after_source: now.source_range,
    };
    assert!(history.apply(&mut project, wrong).is_err());
}

#[test]
fn every_preset_undoes_to_the_byte() {
    for preset in SpeedPreset::ALL {
        let (mut project, a, _) = two_clips();
        let original = canonical(&project);
        let mut history = History::default();
        ramp(&mut project, &mut history, &a, preset);
        no_errors(&project);
        let (_, seg) = project.segment(&a).unwrap();
        let curve = project.materials.speed_curve_of(seg).unwrap();
        assert_eq!(
            seg.target_range.duration,
            curve_target_duration(&curve.points, seg.source_range)
        );
        let ramped = canonical(&project);
        history.undo(&mut project).unwrap();
        assert_eq!(canonical(&project), original, "{preset:?}");
        history.redo(&mut project).unwrap();
        assert_eq!(canonical(&project), ramped, "{preset:?}");
    }
}

#[test]
fn a_linked_sound_takes_the_same_curve_and_stays_in_sync() {
    let (mut project, a, _) = two_clips();
    let mut sound = segment("m", 0, 4_000_000);
    sound.id = "sound".into();
    sound.source_range = TimeRange::new(1_000_000, 4_000_000);
    let mut audio = Track::new(TrackKind::Audio, "A1");
    audio.segments.push(sound);
    project.tracks.push(audio);
    let mut history = History::default();
    let link =
        chukcut_engine::modules::timeline::ops::link(&project, &[a.clone(), "sound".to_string()])
            .unwrap();
    history.apply(&mut project, link).unwrap();

    ramp(&mut project, &mut history, &a, SpeedPreset::Bullet);
    no_errors(&project);
    let (_, picture) = project.segment(&a).unwrap();
    let (_, sound) = project.segment("sound").unwrap();
    assert_eq!(picture.target_range, sound.target_range);
    assert_eq!(
        project.materials.speed_curve_of(picture).map(|c| &c.id),
        project.materials.speed_curve_of(sound).map(|c| &c.id)
    );
    // A curved clip is silent: the mixer leaves it out.
    let planned = chukcut_engine::modules::audio::mixer::plan(&project);
    assert!(planned
        .iter()
        .all(|p| p.segment_id != "sound" && p.segment_id != a));
}

#[test]
fn a_ramped_project_saves_and_opens_unchanged() {
    let (mut project, a, _) = two_clips();
    let mut history = History::default();
    ramp(&mut project, &mut history, &a, SpeedPreset::JumpCut);
    let saved = serde_json::to_string_pretty(&project).unwrap();
    let opened: Project = serde_json::from_str(&saved).expect("the file opens");
    assert_eq!(canonical(&opened), canonical(&project));
    no_errors(&opened);
    // A file written before speed curves existed has no such key at all.
    let (plain, _, _) = two_clips();
    assert!(!serde_json::to_string(&plain)
        .unwrap()
        .contains("speed_curves"));
}

// ---------------------------------------------------------------------------
// Pictures
// ---------------------------------------------------------------------------

/// The counter clip, 2 s of it from the top, on a 320x240 canvas, ramped.
fn counter_ramp(preset: SpeedPreset) -> Option<(Project, String)> {
    let media = support::media().ok()?;
    let mut project = Project::new(
        "speed export",
        CanvasConfig {
            width: 320,
            height: 240,
            background: [0.0, 0.0, 0.0, 1.0],
        },
        30.0,
    );
    project
        .materials
        .videos
        .push(material_for("counter", &media.counter).ok()?);
    let mut track = Track::new(TrackKind::Video, "V1");
    let clip = segment("counter", 0, 2_000_000);
    let id = clip.id.clone();
    track.segments.push(clip);
    project.tracks.push(track);
    let mut history = History::default();
    ramp(&mut project, &mut history, &id, preset);
    Some((project, id))
}

/// The counter frame the decoder should hand back for source instant `t`.
fn counter_frame_at(t: Micros) -> i64 {
    (t as f64 / FRAME + 1e-6).floor() as i64
}

#[test]
fn the_compositor_shows_the_frame_the_curve_reaches() {
    let _ = require_media!();
    let ctx = require_gpu!();
    let Some((project, id)) = counter_ramp(SpeedPreset::Hero) else {
        return;
    };
    let compositor = Compositor::new(ctx);
    let sources: Arc<dyn SourceProvider> = Arc::new(MediaSourceProvider::from_project(&project));
    let (_, seg) = project.segment(&id).unwrap();
    let map = project.materials.time_map(seg);
    let length = seg.target_range.duration;
    let mut seen = Vec::new();
    for n in 0..(length as f64 / FRAME) as i64 {
        let time = (n as f64 * FRAME).round() as Micros + 10;
        let frame = compositor
            .render(&project, time, (320, 240), sources.as_ref())
            .expect("render");
        let got = read_counter_rgba(&frame.data, frame.width, frame.height).expect("a clean frame")
            as i64;
        let want = counter_frame_at(map.source_time_at(time).unwrap());
        assert!(
            (got - want).abs() <= 1,
            "timeline frame {n}: counter {got}, curve says {want}"
        );
        seen.push(got);
    }
    assert!(
        seen.windows(2).all(|w| w[1] >= w[0]),
        "the ramp never plays backwards"
    );
    // The slow section repeats frames; the fast sections skip them.
    assert!(seen.windows(2).any(|w| w[1] == w[0]));
    assert!(seen.windows(2).any(|w| w[1] >= w[0] + 2));
}

#[test]
fn an_export_has_as_many_frames_as_the_curve_is_long() {
    let _ = require_media!();
    let ctx = require_gpu!();
    let Some((project, id)) = counter_ramp(SpeedPreset::Montage) else {
        return;
    };
    let (_, seg) = project.segment(&id).unwrap();
    let length = seg.target_range.duration;
    let map = project.materials.time_map(seg);

    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("speed");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("montage.mp4");
    let _ = std::fs::remove_file(&path);
    let request = ExportRequest {
        output_path: path.to_string_lossy().into_owned(),
        preset_id: None,
        overrides: None,
        hardware: None,
        include_audio: false,
        range: None,
    };
    let settings = resolve_settings(&project, &request).expect("settings");
    // Frames are counted up: a partial last frame is still a frame.
    let expected = ((length as i128 * 30 + 999_999) / 1_000_000) as u64;
    assert_eq!(
        settings.total_frames, expected,
        "the export is as long as the curve"
    );

    let job = ExportJob {
        job_id: "speed".into(),
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
    let sink = FnSink({
        let seen = Arc::new(Mutex::new(0usize));
        move |_| *seen.lock().unwrap() += 1
    });
    let outcome = run_export(&job, &sink).expect("export");
    assert_eq!(outcome.frames, expected);
    let probe = probe_output(&path).expect("probe");
    assert_eq!(
        probe.decoded_frames, expected,
        "counted by decoding the file"
    );

    let mut decoder = VideoDecoder::open(&path).expect("open the export");
    for n in [0u64, 5, expected / 3, expected / 2, expected - 1] {
        let at = (n as f64 * FRAME).round() as i64 + 16_000;
        let frame = decoder.seek_and_decode(at).expect("decode");
        let got = read_counter_rgba(&frame.data, frame.width, frame.height).expect("clean") as i64;
        let time = (n as f64 * FRAME).round() as Micros + 10;
        let want = counter_frame_at(map.source_time_at(time.min(length - 1)).unwrap());
        assert!(
            (got - want).abs() <= 1,
            "export frame {n}: counter {got}, curve says {want}"
        );
    }
    let _ = std::fs::remove_file(&path);
}
