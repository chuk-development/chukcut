//! The talking-head tools end to end, on real files through the real decoder:
//! silence analysis and cutting, voice cleanup read identically by preview and
//! export, and an export brought to a loudness target — checked with FFmpeg's
//! own `ebur128` filter, a meter this codebase did not write.
//!
//! The speech fixture is written by hand as a WAV: a tone where someone talks,
//! digital silence (or quiet noise) where they pause, at known instants. That
//! makes "the cut lands at 1.62 s" a checkable fact rather than a guess about
//! what a recording contains.

mod support;

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use chukcut_engine::modules::audio::decode::FileAudioSource;
use chukcut_engine::modules::export::presets::AudioCodec;
use chukcut_engine::modules::export::{
    resolve_settings, run_export, ExportJob, ExportOverrides, ExportRequest, FnSink,
};
use chukcut_engine::modules::media::MediaSourceProvider;
use chukcut_engine::modules::project::document::{
    AudioMaterial, CanvasConfig, Micros, Project, TimeRange, Track, TrackKind, MICROS_PER_SECOND,
};
use chukcut_engine::modules::render::{Compositor, CompositorConfig, SourceProvider};
use chukcut_engine::modules::silence::commands::{silence_analyse, silence_detect, silence_remove};
use chukcut_engine::modules::silence::SilenceParams;
use chukcut_engine::modules::voice::commands::voice_set_denoise;
use chukcut_engine::state::AppState;

const S: Micros = MICROS_PER_SECOND;
const RATE: u32 = 48_000;

fn scratch() -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("talking_head");
    std::fs::create_dir_all(&dir).expect("create the scratch directory");
    dir
}

/// Keep the denoise cache out of the user's own `~/.cache`.
fn private_cache() {
    // Every test sets the same value, so the race between test threads is
    // harmless.
    std::env::set_var("XDG_CACHE_HOME", scratch().join("cache"));
}

/// A deterministic noise generator, so the fixture is the same every run.
struct Noise(u64);

impl Noise {
    fn next(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0
    }
}

/// Write a stereo 48 kHz WAV of `spans`: `(seconds, speaking)`. Speech is a
/// voiced, swelling harmonic tone at about −16 dBFS RMS; every sample also
/// carries noise at `noise` amplitude.
fn speech_wav(name: &str, spans: &[(f32, bool)], noise: f32) -> (PathBuf, Micros) {
    let path = scratch().join(name);
    let mut rng = Noise(7);
    let mut samples: Vec<i16> = Vec::new();
    let mut t = 0usize;
    for (seconds, speaking) in spans {
        let frames = (seconds * RATE as f32) as usize;
        for _ in 0..frames {
            let time = t as f32 / RATE as f32;
            let mut v = rng.next() * noise;
            if *speaking {
                let swell = 0.6 + 0.4 * (time * 3.0 * std::f32::consts::TAU).sin();
                let voiced: f32 = (1..6)
                    .map(|h| (time * 140.0 * h as f32 * std::f32::consts::TAU).sin() / h as f32)
                    .sum();
                v += 0.18 * swell * voiced;
            }
            let s = (v.clamp(-1.0, 1.0) * 32_767.0) as i16;
            samples.push(s);
            samples.push(s);
            t += 1;
        }
    }
    let data = (samples.len() * 2) as u32;
    let mut bytes = Vec::with_capacity(44 + data as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&RATE.to_le_bytes());
    bytes.extend_from_slice(&(RATE * 4).to_le_bytes());
    bytes.extend_from_slice(&4u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data.to_le_bytes());
    for s in samples {
        bytes.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(&path, bytes).expect("write the fixture");
    (path, (t as i64 * S) / RATE as i64)
}

/// A project holding the fixture as one clip on an audio lane.
fn audio_project(path: &Path, duration: Micros) -> Project {
    let mut project = Project::new(
        "talking head",
        CanvasConfig {
            width: 320,
            height: 240,
            background: [0.0, 0.0, 0.0, 1.0],
        },
        30.0,
    );
    project.materials.audios.push(AudioMaterial {
        id: "voice".into(),
        path: path.to_string_lossy().into_owned(),
        duration,
        sample_rate: RATE,
        channels: 2,
    });
    let mut track = Track::new(TrackKind::Audio, "A1");
    track.segments.push(support::segment("voice", 0, duration));
    project.tracks.push(track);
    project
}

fn open(project: Project) -> Arc<AppState> {
    let state = AppState::new();
    *state.project.write() = Some(project);
    state
}

#[test]
fn pauses_are_found_where_they_are_and_cut_as_one_undo_step() {
    private_cache();
    // talk 1.5 s · pause 1.5 s · talk 1.5 s · pause 0.3 s · talk 1.0 s
    let (path, duration) = speech_wav(
        "pauses.wav",
        &[
            (1.5, true),
            (1.5, false),
            (1.5, true),
            (0.3, false),
            (1.0, true),
        ],
        0.0005,
    );
    let state = open(audio_project(&path, duration));
    let segment = state
        .with_project(|p| p.tracks[0].segments[0].id.clone())
        .unwrap();

    let analysis = silence_analyse(&state, segment.clone(), false, &AtomicBool::new(false))
        .expect("analyse the clip");
    assert_eq!(analysis.envelope.db.len(), 580);
    let params = SilenceParams {
        threshold_db: analysis.suggested_threshold_db,
        min_silence: 500_000,
        padding: 100_000,
        ..Default::default()
    };
    let cuts = silence_detect(&analysis, &params);
    // Only the long pause; the 0.3 s one is natural rhythm.
    assert_eq!(cuts.len(), 1, "cuts: {cuts:?}");
    let cut = cuts[0];
    let near = |a: Micros, b: Micros| (a - b).abs() <= 20_000;
    assert!(near(cut.start, 1_600_000), "cut starts at {}", cut.start);
    assert!(near(cut.end(), 2_900_000), "cut ends at {}", cut.end());

    silence_remove(&state, segment, cuts.clone(), "Remove silences".into()).expect("apply");
    let after = state.with_project(|p| p.duration()).unwrap();
    assert_eq!(after, duration - cut.duration);
    let pieces = state.with_project(|p| p.tracks[0].segments.len()).unwrap();
    assert_eq!(pieces, 2);

    // One undo brings the whole take back.
    chukcut_engine::modules::timeline::commands::timeline_undo(&state).unwrap();
    assert_eq!(state.with_project(|p| p.duration()).unwrap(), duration);
    assert_eq!(
        state.with_project(|p| p.tracks[0].segments.len()).unwrap(),
        1
    );
}

#[test]
fn denoise_is_rendered_in_place_and_heard_the_same_by_preview_and_export() {
    private_cache();
    let (path, duration) = speech_wav("noisy.wav", &[(1.0, true), (1.5, false), (1.0, true)], 0.05);
    let state = open(audio_project(&path, duration));
    let segment = state
        .with_project(|p| p.tracks[0].segments[0].id.clone())
        .unwrap();
    voice_set_denoise(
        &state,
        segment.clone(),
        Some(1.0),
        &AtomicBool::new(false),
        &|_| {},
    )
    .expect("denoise");

    let project = state.with_project(|p| p.clone()).unwrap();
    // The preview's plan reads the cleaned file…
    let plan = chukcut_engine::modules::audio::plan(&project);
    assert_eq!(plan.len(), 1);
    let cleaned = plan[0].path.clone();
    assert_ne!(
        cleaned,
        path.to_string_lossy(),
        "the preview reads the render"
    );
    assert!(cleaned.ends_with(".wav"));

    // …and so does the export's mixer, through the same resolver.
    let (_, seg) = project.segment(&segment).unwrap();
    let effective =
        chukcut_engine::modules::voice::effective_source(&project, seg, &path.to_string_lossy());
    assert_eq!(effective.path, cleaned);

    // The render is exactly as long as the original and quieter in the pause.
    let info = chukcut_engine::modules::media::probe(&cleaned).expect("probe the render");
    assert!(
        (info.duration - duration).abs() <= 1_000,
        "{} vs {duration}",
        info.duration
    );
    let level = |file: &str, range: TimeRange| {
        chukcut_engine::modules::loudness::measure_file(file, range, &AtomicBool::new(false))
            .unwrap()
            .true_peak_db
    };
    let pause = TimeRange::new(1_300_000, 900_000);
    let before = level(&path.to_string_lossy(), pause);
    let after = level(&cleaned, pause);
    assert!(
        after < before - 10.0,
        "noise peak {before:.1} dBTP → {after:.1} dBTP"
    );

    // Turning it off goes back to the original file.
    voice_set_denoise(&state, segment, None, &AtomicBool::new(false), &|_| {}).expect("off");
    let project = state.with_project(|p| p.clone()).unwrap();
    assert_eq!(
        chukcut_engine::modules::audio::plan(&project)[0].path,
        path.to_string_lossy()
    );
}

/// Integrated loudness of `file` according to `ffmpeg -af ebur128`.
fn ffmpeg_integrated(file: &Path) -> Option<f64> {
    let out = std::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-nostats", "-i"])
        .arg(file)
        .args(["-af", "ebur128=peak=true", "-f", "null", "-"])
        .output()
        .ok()?;
    let log = String::from_utf8_lossy(&out.stderr);
    // The summary block at the end: "    I:         -14.0 LUFS".
    let summary = log.rsplit("Summary:").next()?;
    summary.lines().find_map(|line| {
        let line = line.trim();
        line.strip_prefix("I:")
            .and_then(|rest| rest.trim().strip_suffix("LUFS"))
            .and_then(|v| v.trim().parse().ok())
    })
}

#[test]
fn an_export_with_a_loudness_target_measures_on_target() {
    private_cache();
    if std::process::Command::new("ffmpeg")
        .arg("-version")
        .output()
        .is_err()
    {
        eprintln!("skipping: no ffmpeg to measure with");
        return;
    }
    let ctx = require_gpu!();
    let (path, duration) = speech_wav(
        "quiet.wav",
        &[(2.0, true), (0.5, false), (2.5, true)],
        0.002,
    );
    let mut project = audio_project(&path, duration);
    // Quieter still: a voice recorded far from the microphone.
    project.tracks[0].segments[0].volume = 0.3;

    for target in [-14.0f32, -23.0] {
        let output = scratch().join(format!("loud{}.mp4", -target as i32));
        let request = ExportRequest {
            output_path: output.to_string_lossy().into_owned(),
            preset_id: None,
            overrides: Some(ExportOverrides {
                audio_codec: Some(AudioCodec::Aac),
                loudness_target: Some(target),
                ..Default::default()
            }),
            hardware: None,
            include_audio: true,
            range: None,
        };
        let settings = resolve_settings(&project, &request).expect("settings");
        assert_eq!(settings.loudness_target, Some(target));
        let output = settings.output_path.clone();
        let compositor = Arc::new(Compositor::with_config(
            Arc::clone(&ctx),
            CompositorConfig {
                strict_sources: true,
                ..Default::default()
            },
        ));
        let sources: Arc<dyn SourceProvider> =
            Arc::new(MediaSourceProvider::from_project(&project));
        let job = ExportJob {
            job_id: "loudness".into(),
            project: project.clone(),
            settings,
            compositor,
            sources,
            audio: Arc::new(FileAudioSource),
            cancel: Arc::new(AtomicBool::new(false)),
        };
        run_export(&job, &FnSink(|_| {})).expect("export");
        let measured = ffmpeg_integrated(&output).expect("ffmpeg measured the export");
        assert!(
            (measured - target as f64).abs() <= 1.0,
            "target {target} LUFS, ffmpeg measured {measured} LUFS"
        );
    }
}
