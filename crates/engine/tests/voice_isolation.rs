//! Isolate voice, end to end: the setting as one undo step, the preview
//! playing the clip as it was until the isolated sound exists, and — where
//! the ML worker and HTDemucs are installed — a real separation of
//! synthetic speech from synthetic music, measured against the speech it
//! was mixed from.
//!
//! The speech is FFmpeg's `flite` voice reading a sentence, the music a
//! chord with a beat: both generated, so "how much of the music is left" is
//! a number this test can compute.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use chukcut_engine::modules::audio::decode::{AudioClipReader, ClipReader};
use chukcut_engine::modules::project::document::{
    AudioMaterial, CanvasConfig, Project, Segment, TimeRange, Track, TrackKind, Transform,
};
use chukcut_engine::modules::timeline::commands::timeline_undo;
use chukcut_engine::modules::voice::commands::{
    voice_isolation_missing, voice_isolation_render, voice_set_isolation, IsolationSetting,
};
use chukcut_engine::modules::voice::isolate::{self, Keep};
use chukcut_engine::modules::voice::{cleanup_of, effective_source};
use chukcut_engine::state::AppState;

const RATE: u32 = 44_100;

fn scratch() -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("voice_isolation");
    std::fs::create_dir_all(&dir).expect("create the scratch directory");
    dir
}

/// Keep the renders out of the user's cache, but let the ML directory (the
/// models, the runtime) be the real one: a symlink from the private cache.
fn private_cache() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let real = chukcut_engine::modules::ml::root();
        let cache = scratch().join("cache");
        let _ = std::fs::remove_dir_all(cache.join("chukcut/voice"));
        std::fs::create_dir_all(cache.join("chukcut")).unwrap();
        let link = cache.join("chukcut/ml");
        if !link.exists() {
            let _ = std::os::unix::fs::symlink(&real, &link);
        }
        std::env::set_var("XDG_CACHE_HOME", &cache);
    });
}

fn ffmpeg(args: &[&str]) -> bool {
    Command::new("ffmpeg")
        .args(["-loglevel", "error", "-y"])
        .args(args)
        .stdout(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// The speech alone, the music alone, and the two mixed, 8 s of 44.1 kHz
/// stereo each.
fn fixtures() -> Option<(PathBuf, PathBuf, PathBuf)> {
    // Both tests want them; one makes them while the other waits.
    static MAKING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _making = MAKING.lock().unwrap_or_else(|e| e.into_inner());
    let dir = scratch();
    let (speech, music, mix) = (
        dir.join("speech.wav"),
        dir.join("music.wav"),
        dir.join("mix.wav"),
    );
    if !mix.exists() {
        let text = "flite=text='The quick brown fox jumps over the lazy dog. \
                    Every frame of this clip is generated, so the test knows \
                    exactly what was said and when.':voice=slt";
        let ok = ffmpeg(&[
            "-f",
            "lavfi",
            "-i",
            text,
            "-af",
            "apad",
            "-t",
            "8",
            "-ar",
            "44100",
            "-ac",
            "2",
            speech.to_str()?,
        ]) && ffmpeg(&[
            "-f",
            "lavfi",
            "-i",
            "aevalsrc='0.08*sin(2*PI*220*t)+0.06*sin(2*PI*277.2*t)+0.06*sin(2*PI*329.6*t)\
             +0.05*sin(2*PI*110*t)+0.25*exp(-30*mod(t,0.5))*sin(2*PI*60*t)':s=44100:d=8",
            "-ac",
            "2",
            music.to_str()?,
        ]) && ffmpeg(&[
            "-i",
            speech.to_str()?,
            "-i",
            music.to_str()?,
            "-filter_complex",
            "amix=inputs=2:normalize=0",
            "-ar",
            "44100",
            "-ac",
            "2",
            mix.to_str()?,
        ]);
        if !ok {
            return None;
        }
    }
    Some((speech, music, mix))
}

fn read(path: &Path) -> Vec<f32> {
    let mut reader = AudioClipReader::open(path, RATE, 2).expect("open");
    let frames = 8 * RATE as usize;
    let mut out = vec![0.0f32; frames * 2];
    reader.read(0, frames, &mut out).expect("read");
    out
}

/// How far `estimate` is from `truth`, as a signal-to-distortion ratio in
/// dB: higher is closer.
fn sdr(truth: &[f32], estimate: &[f32]) -> f32 {
    let signal: f64 = truth.iter().map(|v| (*v as f64).powi(2)).sum();
    let error: f64 = truth
        .iter()
        .zip(estimate)
        .map(|(a, b)| (*a as f64 - *b as f64).powi(2))
        .sum();
    (10.0 * (signal / error.max(1e-12)).log10()) as f32
}

fn state(mix: &Path) -> Arc<AppState> {
    let mut project = Project::new("iso", CanvasConfig::default(), 30.0);
    project.materials.audios.push(AudioMaterial {
        id: "a".into(),
        path: mix.to_string_lossy().into(),
        duration: 8_000_000,
        sample_rate: RATE,
        channels: 2,
    });
    let mut track = Track::new(TrackKind::Audio, "A1");
    track.segments.push(Segment {
        id: "talk".into(),
        material_id: "a".into(),
        target_range: TimeRange::new(0, 8_000_000),
        source_range: TimeRange::new(0, 8_000_000),
        render_index: 0,
        speed: 1.0,
        volume: 1.0,
        transform: Transform::default(),
        crop: None,
        extras: Vec::new(),
        keyframes: Vec::new(),
    });
    project.tracks.push(track);
    let state = AppState::new();
    *state.project.write() = Some(project);
    state
}

fn heard(state: &AppState) -> String {
    state
        .with_project(|p| {
            let (_, s) = p.segment("talk").unwrap();
            effective_source(p, s, &p.materials.audios[0].path).path
        })
        .unwrap()
}

#[test]
fn the_setting_is_one_undo_step_and_the_clip_plays_dry_until_rendered() {
    private_cache();
    let Some((_, _, mix)) = fixtures() else {
        eprintln!("skipping: ffmpeg could not generate the fixtures");
        return;
    };
    let state = state(&mix);
    voice_set_isolation(
        &state,
        "talk".into(),
        Some(IsolationSetting {
            strength: 0.8,
            keep: Keep::Voice,
        }),
    )
    .unwrap();
    let setting = state
        .with_project(|p| cleanup_of(p, p.segment("talk").unwrap().1).unwrap().1)
        .unwrap()
        .isolate
        .unwrap();
    assert_eq!(setting.strength, 0.8);
    assert_eq!(setting.model, isolate::current_model());
    // Not rendered: the preview plays the original, and the app is told.
    assert_eq!(heard(&state), mix.to_string_lossy());
    assert_eq!(voice_isolation_missing(&state), vec!["talk".to_string()]);

    // A render appearing in the cache is picked up by the next plan.
    let cached = isolate::cache_path(&mix.to_string_lossy(), &setting);
    std::fs::create_dir_all(cached.parent().unwrap()).unwrap();
    std::fs::copy(&mix, &cached).unwrap();
    assert_eq!(heard(&state), cached.to_string_lossy());
    assert!(voice_isolation_missing(&state).is_empty());
    std::fs::remove_file(&cached).unwrap();

    let error = voice_set_isolation(
        &state,
        "talk".into(),
        Some(IsolationSetting {
            strength: 1.5,
            keep: Keep::Voice,
        }),
    )
    .err()
    .expect("refused");
    assert!(error.contains("between 0 and 1"), "{error}");
    timeline_undo(&state).unwrap();
    assert!(state
        .with_project(|p| cleanup_of(p, p.segment("talk").unwrap().1))
        .unwrap()
        .is_none());
}

/// The model itself, where it is installed.
#[test]
fn htdemucs_takes_the_music_out_of_the_speech() {
    use chukcut_engine::modules::ml;
    use chukcut_ml_worker::registry;
    private_cache();
    let root = ml::root();
    let ready = ml::worker::binary().is_some()
        && registry::model_present(&root, registry::model("htdemucs-vocals").unwrap())
        && (registry::preferred_runtime(&root).is_some()
            || std::env::var_os("CHUKCUT_ORT_DYLIB").is_some());
    if !ready {
        eprintln!("skipping: the ML worker, a runtime or HTDemucs is not installed");
        return;
    }
    let Some((speech, music, mix)) = fixtures() else {
        eprintln!("skipping: ffmpeg could not generate the fixtures");
        return;
    };
    let (speech, music, mixed) = (read(&speech), read(&music), read(&mix));
    let state = state(&mix);
    let never = AtomicBool::new(false);

    voice_set_isolation(
        &state,
        "talk".into(),
        Some(IsolationSetting {
            strength: 1.0,
            keep: Keep::Voice,
        }),
    )
    .unwrap();
    let done = voice_isolation_render(&state, "talk".into(), &never, &|_| {}).unwrap();
    eprintln!("8 s isolated in {:.2} s", done.seconds);
    assert_eq!(heard(&state), done.path);
    let voice = read(Path::new(&done.path));
    let (before, after) = (sdr(&speech, &mixed), sdr(&speech, &voice));
    eprintln!("speech SDR: mix {before:.1} dB, isolated {after:.1} dB");
    assert!(after > before + 6.0, "{before:.1} -> {after:.1} dB");

    // Keep the background: the music, near enough, without the voice.
    voice_set_isolation(
        &state,
        "talk".into(),
        Some(IsolationSetting {
            strength: 1.0,
            keep: Keep::Background,
        }),
    )
    .unwrap();
    let started = std::time::Instant::now();
    let done = voice_isolation_render(&state, "talk".into(), &never, &|_| {}).unwrap();
    assert!(
        started.elapsed().as_secs_f32() < 5.0,
        "the stem is cached; only the mix is made"
    );
    let background = read(Path::new(&done.path));
    let (before, after) = (sdr(&music, &mixed), sdr(&music, &background));
    eprintln!("music SDR: mix {before:.1} dB, background {after:.1} dB");
    assert!(after > before + 6.0, "{before:.1} -> {after:.1} dB");

    // An export's own step makes a missing render again.
    std::fs::remove_file(&done.path).unwrap();
    let project = state.project.read().clone().unwrap();
    chukcut_engine::modules::voice::denoise::ensure_rendered(&project, &never).unwrap();
    assert!(Path::new(&done.path).is_file());
}
