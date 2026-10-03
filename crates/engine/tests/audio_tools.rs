//! The audio tools end to end, on real files through the real decoder:
//!
//! - a pitch-preserving speed change and a speed curve are heard (a curved
//!   clip used to be muted), at the source's pitch, and the preview — reading
//!   the cached render through the real mixer — gives the same samples as the
//!   export;
//! - the equaliser matches FFmpeg's own `lowshelf`/`equalizer`/`highshelf`
//!   filters, a filter bank this codebase did not write;
//! - FFmpeg's `rubberband` filter, an independent time stretcher, agrees on
//!   the pitch of a stretched tone;
//! - auto-ducking finds FFmpeg-synthesised speech (`flite`) and writes a dip
//!   that an export measures;
//! - a recorded take lands on a new lane at the playhead in one undo step.
//!
//! Fixtures are generated into the test's scratch directory; tests that need
//! an FFmpeg feature this machine's build lacks say so and skip.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use chukcut_engine::modules::audio::decode::{
    AudioClipReader, ClipReader, FileAudioSource, FileClipFactory,
};
use chukcut_engine::modules::audio::{plan, TimelineMixer};
use chukcut_engine::modules::audiofx::commands::{
    audiofx_add, audiofx_duck, audiofx_place_take, audiofx_render, audiofx_set_param,
};
use chukcut_engine::modules::audiofx::dsp::apply_chain;
use chukcut_engine::modules::audiofx::record::TakeWriter;
use chukcut_engine::modules::audiofx::{AudioEffect, DuckParams};
use chukcut_engine::modules::export::audio::{frames_for, mix_timeline};
use chukcut_engine::modules::project::document::{
    AnimatableProperty, AudioMaterial, CanvasConfig, Micros, Project, Segment, TimeRange, Track,
    TrackKind, Transform, MICROS_PER_SECOND,
};
use chukcut_engine::modules::project::{
    speed::curve_target_duration, SpeedCurveMaterial, SpeedPoint,
};
use chukcut_engine::state::AppState;

const S: Micros = MICROS_PER_SECOND;
const RATE: u32 = 48_000;

fn scratch() -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("audio_tools");
    std::fs::create_dir_all(&dir).expect("create the scratch directory");
    dir
}

/// Keep rendered audio out of the user's own `~/.cache`. Every test sets the
/// same value, so the race between test threads is harmless.
fn private_cache() {
    std::env::set_var("XDG_CACHE_HOME", scratch().join("cache"));
}

fn ffmpeg(args: &[&str]) -> bool {
    Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(args)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// A fixture made by FFmpeg from a lavfi source, as 48 kHz stereo f32 WAV.
fn lavfi(name: &str, source: &str) -> Option<PathBuf> {
    let path = scratch().join(name);
    if path.is_file() {
        return Some(path);
    }
    let ok = ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        source,
        "-ar",
        "48000",
        "-ac",
        "2",
        "-c:a",
        "pcm_f32le",
        path.to_str().unwrap(),
    ]);
    ok.then_some(path)
}

fn decode(path: &Path, frames: usize) -> Vec<f32> {
    let mut reader = AudioClipReader::open(path, RATE, 2).expect("open");
    let mut out = vec![0.0; frames * 2];
    reader.read(0, frames, &mut out).expect("read");
    out
}

fn rms(v: &[f32]) -> f64 {
    (v.iter().map(|s| (*s as f64).powi(2)).sum::<f64>() / v.len().max(1) as f64).sqrt()
}

/// The strongest frequency in the left channel of `v` (interleaved stereo),
/// by zero-crossings of a band-limited tone: plenty for a pure sine.
fn tone_frequency(v: &[f32]) -> f64 {
    let left: Vec<f32> = v.as_chunks::<2>().0.iter().map(|f| f[0]).collect();
    let mut crossings = Vec::new();
    for i in 1..left.len() {
        if left[i - 1] <= 0.0 && left[i] > 0.0 {
            let t = (i - 1) as f64 + (-left[i - 1] / (left[i] - left[i - 1])) as f64;
            crossings.push(t);
        }
    }
    let n = crossings.len();
    assert!(n > 10, "no tone to measure");
    (n - 1) as f64 / (crossings[n - 1] - crossings[0]) * RATE as f64
}

fn project_with(path: &Path, duration: Micros) -> Project {
    let mut project = Project::new(
        "audio tools",
        CanvasConfig {
            width: 320,
            height: 240,
            background: [0.0, 0.0, 0.0, 1.0],
        },
        30.0,
    );
    project.materials.audios.push(AudioMaterial {
        id: "tone".into(),
        path: path.to_string_lossy().into_owned(),
        duration,
        sample_rate: RATE,
        channels: 2,
    });
    project
}

fn segment(id: &str, material: &str, target: TimeRange, source: TimeRange) -> Segment {
    Segment {
        id: id.into(),
        material_id: material.into(),
        target_range: target,
        source_range: source,
        render_index: 0,
        speed: 1.0,
        volume: 1.0,
        transform: Transform::default(),
        crop: None,
        extras: Vec::new(),
        keyframes: Vec::new(),
    }
}

/// The preview mixer's output over `[0, frames)` with every render already in
/// the cache, at the export's rate.
fn preview(project: &Project, frames: usize) -> Vec<f32> {
    let mut mixer = TimelineMixer::new(RATE, Arc::new(FileClipFactory));
    mixer.set_plan(plan(project));
    let mut out = vec![0.0; frames * 2];
    for (i, block) in out.chunks_mut(1_024).enumerate() {
        mixer.fill((i * 512) as i64, block);
    }
    out
}

/// The export mixer's output, unclamped differences aside.
fn export(project: &Project) -> Vec<f32> {
    mix_timeline(project, &FileAudioSource, RATE, 2, &AtomicBool::new(false)).expect("mix")
}

fn state_with(project: Project) -> Arc<AppState> {
    let state = AppState::new();
    *state.project.write() = Some(project);
    state
}

fn assert_same(preview: &[f32], export: &[f32], what: &str) {
    assert_eq!(preview.len(), export.len());
    // The preview soft-limits above 0.7 and the export clamps at 1; these
    // fixtures stay below both, so the two must agree sample for sample.
    let worst = preview
        .iter()
        .zip(export)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(worst < 1e-5, "{what}: preview and export differ by {worst}");
}

#[test]
fn double_speed_keeps_the_pitch_and_the_preview_plays_what_the_export_writes() {
    private_cache();
    let Some(path) = lavfi(
        "a440.wav",
        "sine=frequency=440:duration=8:sample_rate=48000",
    ) else {
        eprintln!("skipping: no ffmpeg");
        return;
    };
    let mut project = project_with(&path, 8 * S);
    let mut seg = segment(
        "clip",
        "tone",
        TimeRange::new(0, 2 * S),
        TimeRange::new(S, 4 * S),
    );
    seg.speed = 2.0;
    seg.volume = 0.5;
    let mut lane = Track::new(TrackKind::Audio, "A1");
    lane.segments.push(seg);
    project.tracks.push(lane);

    let state = state_with(project.clone());
    let cached = audiofx_render(&state, "clip".into(), &AtomicBool::new(false))
        .expect("render")
        .expect("a speed change is rendered");
    assert!(Path::new(&cached).is_file());

    let exported = export(&project);
    assert_eq!(exported.len(), frames_for(2 * S, RATE) * 2);
    let f = tone_frequency(&exported[2 * 24_000..2 * 72_000]);
    assert!((f - 440.0).abs() < 1.0, "the pitch moved to {f} Hz");

    let previewed = preview(&project, frames_for(2 * S, RATE));
    assert_same(&previewed, &exported, "2x");

    // FFmpeg's rubberband, where the build has it, agrees on the pitch: an
    // independent stretcher as the reference for "kept".
    let reference = scratch().join("a440-rubberband.wav");
    if ffmpeg(&[
        "-i",
        path.to_str().unwrap(),
        "-af",
        "rubberband=tempo=2",
        "-c:a",
        "pcm_f32le",
        reference.to_str().unwrap(),
    ]) {
        let theirs = decode(&reference, 96_000);
        let g = tone_frequency(&theirs[2 * 24_000..2 * 72_000]);
        assert!((f - g).abs() < 1.0, "ours {f} Hz, rubberband {g} Hz");
    }
}

#[test]
fn a_clip_on_a_speed_curve_is_heard_at_its_pitch_in_preview_and_export_alike() {
    private_cache();
    let Some(path) = lavfi(
        "a330.wav",
        "sine=frequency=330:duration=10:sample_rate=48000",
    ) else {
        eprintln!("skipping: no ffmpeg");
        return;
    };
    let mut project = project_with(&path, 10 * S);
    let points = vec![
        SpeedPoint {
            source: S,
            speed: 0.5,
        },
        SpeedPoint {
            source: 3 * S,
            speed: 3.0,
        },
        SpeedPoint {
            source: 5 * S,
            speed: 1.0,
        },
    ];
    let source = TimeRange::new(S, 5 * S);
    let length = curve_target_duration(&points, source);
    project.materials.speed_curves.push(SpeedCurveMaterial {
        id: "ramp".into(),
        preset: None,
        points,
    });
    let mut seg = segment("clip", "tone", TimeRange::new(S / 2, length), source);
    seg.extras.push("ramp".into());
    seg.volume = 0.5;
    let mut lane = Track::new(TrackKind::Audio, "A1");
    lane.segments.push(seg);
    project.tracks.push(lane);

    let state = state_with(project.clone());
    audiofx_render(&state, "clip".into(), &AtomicBool::new(false))
        .expect("render")
        .expect("a curve is rendered");

    let exported = export(&project);
    let frames = exported.len() / 2;
    let body = &exported[2 * frames_for(S, RATE)..2 * (frames - frames_for(S, RATE))];
    // lavfi's sine is 1/8 of full scale, at half volume: RMS 0.044.
    assert!(
        rms(body) > 0.03,
        "a curved clip is no longer muted ({})",
        rms(body)
    );
    let f = tone_frequency(&exported[2 * frames_for(S, RATE)..2 * frames_for(2 * S, RATE)]);
    assert!((f - 330.0).abs() < 2.0, "{f} Hz in the slow part");

    let previewed = preview(&project, frames);
    assert_same(&previewed, &exported, "curve");
}

#[test]
fn the_equaliser_matches_ffmpegs_filters() {
    // Three tones the three bands act on, through our EQ and through FFmpeg's
    // RBJ biquads with the same corners, gains and widths.
    let Some(path) = lavfi(
        "three-tones.wav",
        "aevalsrc=0.2*sin(2*PI*60*t)+0.2*sin(2*PI*1000*t)+0.2*sin(2*PI*12000*t):s=48000:d=2",
    ) else {
        eprintln!("skipping: no ffmpeg");
        return;
    };
    let reference = scratch().join("three-tones-ffmpeg.wav");
    if !ffmpeg(&[
        "-i",
        path.to_str().unwrap(),
        "-af",
        "lowshelf=f=120:g=6:t=q:w=0.707,equalizer=f=1000:t=q:w=0.8:g=-9,highshelf=f=8000:g=4:t=q:w=0.707",
        "-c:a",
        "pcm_f32le",
        reference.to_str().unwrap(),
    ]) {
        eprintln!("skipping: this FFmpeg has no shelf filters");
        return;
    }
    let input = decode(&path, 96_000);
    let theirs = decode(&reference, 96_000);
    let mut ours = input.clone();
    let mut eq = AudioEffect::new("eq3");
    eq.params.insert("low".into(), 6.0);
    eq.params.insert("mid".into(), -9.0);
    eq.params.insert("high".into(), 4.0);
    apply_chain(&mut ours, 2, RATE, &[eq]);

    // After the filters settle, the two outputs are the same signal.
    let a = &ours[2 * 4_800..];
    let b = &theirs[2 * 4_800..];
    let diff: Vec<f32> = a.iter().zip(b).map(|(x, y)| x - y).collect();
    let ratio = 20.0 * (rms(&diff) / rms(b)).log10();
    assert!(ratio < -40.0, "ours and FFmpeg's differ at {ratio} dB");
    // And the EQ did something.
    assert!((20.0 * (rms(a) / rms(&input[2 * 4_800..])).log10()).abs() > 1.0);
}

#[test]
fn effects_on_a_clip_sound_the_same_in_preview_and_export() {
    private_cache();
    let Some(path) = lavfi(
        "a523.wav",
        "sine=frequency=523:duration=4:sample_rate=48000",
    ) else {
        eprintln!("skipping: no ffmpeg");
        return;
    };
    let mut project = project_with(&path, 4 * S);
    let mut seg = segment(
        "clip",
        "tone",
        TimeRange::new(0, 3 * S),
        TimeRange::new(0, 3 * S),
    );
    seg.volume = 0.4;
    let mut lane = Track::new(TrackKind::Audio, "A1");
    lane.segments.push(seg);
    project.tracks.push(lane);
    let state = state_with(project);
    let (_, reverb) = audiofx_add(&state, "clip".into(), "reverb".into()).unwrap();
    audiofx_set_param(&state, "clip".into(), reverb, "mix".into(), 0.5).unwrap();
    audiofx_add(&state, "clip".into(), "voice_telephone".into()).unwrap();
    audiofx_add(&state, "clip".into(), "voice_deep".into()).unwrap();
    audiofx_render(&state, "clip".into(), &AtomicBool::new(false))
        .unwrap()
        .expect("effects are rendered");
    let project = state.project.read().clone().unwrap();

    let exported = export(&project);
    let dry = decode(&path, exported.len() / 2);
    let changed = exported
        .iter()
        .zip(&dry)
        .map(|(a, b)| (a - b * 0.4).abs())
        .fold(0.0f32, f32::max);
    assert!(changed > 0.05, "the effects changed the sound");
    let previewed = preview(&project, exported.len() / 2);
    assert_same(&previewed, &exported, "effect stack");
}

#[test]
fn ducking_turns_the_music_down_where_flite_speaks() {
    private_cache();
    let Some(music) = lavfi(
        "music.wav",
        "sine=frequency=220:duration=12:sample_rate=48000",
    ) else {
        eprintln!("skipping: no ffmpeg");
        return;
    };
    // Speech from 3 s: two seconds of silence, then flite, then silence.
    let speech = scratch().join("speech.wav");
    if !speech.is_file()
        && !ffmpeg(&[
            "-f",
            "lavfi",
            "-i",
            "flite=text='The quick brown fox jumps over the lazy dog. Then it sat down for a while.'",
            "-af",
            "adelay=3000|3000,apad=whole_dur=12",
            "-ar",
            "48000",
            "-ac",
            "2",
            "-c:a",
            "pcm_f32le",
            speech.to_str().unwrap(),
        ])
    {
        eprintln!("skipping: this FFmpeg has no flite");
        return;
    }
    let mut project = project_with(&music, 12 * S);
    project.materials.audios.push(AudioMaterial {
        id: "speech".into(),
        path: speech.to_string_lossy().into_owned(),
        duration: 12 * S,
        sample_rate: RATE,
        channels: 2,
    });
    let mut voice = Track::new(TrackKind::Audio, "Voice");
    voice.segments.push(segment(
        "speech",
        "speech",
        TimeRange::new(0, 12 * S),
        TimeRange::new(0, 12 * S),
    ));
    let mut bed = Track::new(TrackKind::Audio, "Music");
    let mut m = segment(
        "music",
        "tone",
        TimeRange::new(0, 12 * S),
        TimeRange::new(0, 12 * S),
    );
    m.volume = 0.5;
    bed.segments.push(m);
    project.tracks.push(voice);
    project.tracks.push(bed);
    let state = state_with(project);

    let params = DuckParams {
        depth_db: 12.0,
        attack: 200_000,
        release: 400_000,
        threshold_db: -45.0,
    };
    audiofx_duck(&state, "music".into(), params, &AtomicBool::new(false)).expect("ducked");
    let mut project = state.project.read().clone().unwrap();
    let (_, music_seg) = project.segment("music").unwrap();
    let keys = music_seg
        .keyframes
        .iter()
        .find(|k| k.property == AnimatableProperty::Volume)
        .expect("ducking writes volume keyframes")
        .clone();
    assert!(
        (keys.sample(S).unwrap() - 1.0).abs() < 1e-3,
        "full before the speech"
    );
    let floor = 10f32.powf(-12.0 / 20.0);
    assert!(
        (keys.sample(4 * S).unwrap() - floor).abs() < 1e-3,
        "down by the depth while flite speaks"
    );

    // Measured in an export of the music alone.
    project.tracks[0].muted = true;
    let mixed = export(&project);
    let at = |t: Micros| {
        let i = frames_for(t, RATE) * 2;
        rms(&mixed[i..i + 2 * 4_800])
    };
    let dip = 20.0 * (at(4 * S) / at(S)).log10();
    assert!((dip + 12.0).abs() < 0.5, "the music dipped by {dip} dB");

    // One undo step takes it all back.
    chukcut_engine::modules::timeline::commands::timeline_undo(&state).unwrap();
    let project = state.project.read().clone().unwrap();
    let (_, music_seg) = project.segment("music").unwrap();
    assert!(music_seg.keyframes.is_empty());
}

#[test]
fn a_take_lands_on_a_new_lane_at_the_playhead_in_one_undo_step() {
    let dir = scratch().join("takes");
    let path = chukcut_engine::modules::audiofx::record::take_path(&dir);
    let mut writer = TakeWriter::create(&path, RATE, 1, Duration::from_millis(250)).unwrap();
    let tone: Vec<f32> = (0..3 * RATE as usize / 2)
        .map(|i| 0.3 * (i as f32 * 0.05).sin())
        .collect();
    writer.push(&tone).unwrap();
    let take = writer.finish().unwrap();
    assert_eq!(
        take.duration,
        (5 * S) / 4,
        "1.5 s minus the 0.25 s count-in"
    );

    let mut project = project_with(&path, S);
    project.materials.audios.clear();
    project.tracks.push(Track::new(TrackKind::Video, "V1"));
    project.tracks.push(Track::new(TrackKind::Audio, "A1"));
    let state = state_with(project);
    audiofx_place_take(&state, &take, 2 * S).expect("placed");
    let project = state.project.read().clone().unwrap();
    assert_eq!(project.tracks.len(), 3);
    let lane = &project.tracks[2];
    assert_eq!(lane.kind, TrackKind::Audio);
    assert_eq!(lane.segments.len(), 1);
    let clip = &lane.segments[0];
    assert_eq!(clip.target_range.start, 2 * S);
    assert!((clip.target_range.duration - take.duration).abs() < 1_000);
    assert_eq!(project.materials.audios.len(), 1);

    chukcut_engine::modules::timeline::commands::timeline_undo(&state).unwrap();
    let project = state.project.read().clone().unwrap();
    assert_eq!(project.tracks.len(), 2, "the lane goes with the take");
}
