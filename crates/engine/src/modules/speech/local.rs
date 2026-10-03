//! Offline transcription with whisper.cpp.
//!
//! Compiled only with the `local-whisper` feature, because building
//! whisper.cpp costs a CMake run. Without it [`transcribe`] says how to get it.

use std::path::Path;
use std::sync::atomic::AtomicBool;

use crate::modules::captions::Transcript;

/// Whether this build carries whisper.cpp.
pub const AVAILABLE: bool = cfg!(feature = "local-whisper");

#[cfg(not(feature = "local-whisper"))]
pub fn transcribe(
    _model: &Path,
    _samples: &[f32],
    _language: Option<&str>,
    _progress: &(dyn Fn(f32) + Sync),
    _cancel: &AtomicBool,
) -> Result<Transcript, String> {
    Err("this build has no offline transcription; build with the `local-whisper` feature, or use a cloud account".to_string())
}

/// Run whisper.cpp over `samples` (16 kHz mono) and return one timed word per
/// segment.
///
/// Word timing comes from whisper.cpp's own token timestamps with
/// `max_len = 1` and `split_on_word`, which makes every segment one word — the
/// documented way to get word-level output from it. `progress` gets 0..1.
///
/// The model is loaded per call. Loading is a second or two against a
/// transcription that takes tens of seconds, and holding a gigabyte of model
/// in memory for an editor that captions once a session is the worse trade.
#[cfg(feature = "local-whisper")]
pub fn transcribe(
    model: &Path,
    samples: &[f32],
    language: Option<&str>,
    progress: &(dyn Fn(f32) + Sync),
    cancel: &AtomicBool,
) -> Result<Transcript, String> {
    use std::sync::atomic::{AtomicI32, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

    use crate::modules::captions::TimedWord;

    // whisper.cpp prints to stderr by default; this routes it nowhere.
    whisper_rs::install_logging_hooks();

    let context = WhisperContext::new_with_params(model, WhisperContextParameters::default())
        .map_err(|e| {
            format!("the model could not be loaded ({e}); delete it to download it again")
        })?;
    let mut state = context
        .create_state()
        .map_err(|e| format!("whisper could not start: {e}"))?;

    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(1, 8);
    params.set_n_threads(threads as i32);
    params.set_language(Some(language.filter(|l| !l.is_empty()).unwrap_or("auto")));
    params.set_token_timestamps(true);
    params.set_max_len(1);
    params.set_split_on_word(true);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_special(false);
    params.set_print_timestamps(false);

    // whisper.cpp's callbacks must be `'static`, and `progress` and `cancel`
    // are borrowed. So the callbacks talk to two shared atomics, and a scoped
    // thread relays between those and the borrowed ones while whisper runs.
    let percent = Arc::new(AtomicI32::new(0));
    let abort = Arc::new(AtomicBool::new(false));
    {
        let percent = Arc::clone(&percent);
        params.set_progress_callback_safe(move |p: i32| percent.store(p, Ordering::Relaxed));
    }
    // Not `set_abort_callback_safe`: in whisper-rs 0.16 its trampoline casts
    // the user data to the closure type while what it stored is a boxed trait
    // object, so the callback reads garbage and whisper.cpp aborts the encoder
    // at once ("error -6"). The raw form with a pointer to the flag is simple
    // and sound: `abort` outlives `state.full`, which is the only caller.
    unsafe extern "C" fn should_abort(user_data: *mut std::ffi::c_void) -> bool {
        // SAFETY: `user_data` is `Arc::as_ptr(&abort)`, alive for the call.
        unsafe { (*(user_data as *const AtomicBool)).load(Ordering::Relaxed) }
    }
    // SAFETY: see `should_abort`.
    unsafe {
        params.set_abort_callback(Some(should_abort));
        params.set_abort_callback_user_data(Arc::as_ptr(&abort) as *mut std::ffi::c_void);
    }

    let finished = AtomicBool::new(false);
    let result = std::thread::scope(|scope| {
        scope.spawn(|| {
            while !finished.load(Ordering::Relaxed) {
                if cancel.load(Ordering::Relaxed) {
                    abort.store(true, Ordering::Relaxed);
                }
                progress(percent.load(Ordering::Relaxed) as f32 / 100.0);
                std::thread::sleep(Duration::from_millis(250));
            }
        });
        let result = state.full(params, samples);
        finished.store(true, Ordering::Relaxed);
        result
    });
    if cancel.load(Ordering::Relaxed) {
        return Err("cancelled".to_string());
    }
    result.map_err(|e| format!("whisper failed: {e}"))?;

    let centis = |t: i64| t.max(0) * 10_000;
    let mut words = Vec::new();
    for segment in state.as_iter() {
        let text = segment
            .to_str_lossy()
            .map(|t| t.trim().to_string())
            .unwrap_or_default();
        // Non-speech markers: "[MUSIC]", "(laughs)", "[BLANK_AUDIO]".
        if text.is_empty()
            || (text.starts_with('[') && text.ends_with(']'))
            || (text.starts_with('(') && text.ends_with(')'))
        {
            continue;
        }
        words.push(TimedWord {
            text,
            start: centis(segment.start_timestamp()),
            end: centis(segment.end_timestamp()),
        });
    }
    let language = whisper_rs::get_lang_str(state.full_lang_id_from_state()).map(str::to_string);
    progress(1.0);
    Ok(Transcript {
        language,
        words,
        segments: Vec::new(),
    })
}
