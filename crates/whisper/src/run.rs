//! Running whisper.cpp: the same code in the editor (CPU) and in the CUDA
//! helper.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::{Device, Transcription, Word};

/// The device whisper.cpp will use in this build and on this machine: the
/// first GPU ggml's CUDA backend found, else the CPU. Without the `cuda`
/// feature there is no GPU backend and this is always the CPU.
pub fn device() -> Device {
    #[cfg(feature = "cuda")]
    {
        use std::ffi::CStr;
        use whisper_rs::whisper_rs_sys as sys;
        // SAFETY: ggml's device registry is initialised on first use and
        // lives for the process; the names are static C strings it owns.
        unsafe {
            for index in 0..sys::ggml_backend_dev_count() {
                let device = sys::ggml_backend_dev_get(index);
                if device.is_null()
                    || sys::ggml_backend_dev_type(device)
                        != sys::ggml_backend_dev_type_GGML_BACKEND_DEVICE_TYPE_GPU
                {
                    continue;
                }
                let text = |p: *const std::os::raw::c_char| {
                    if p.is_null() {
                        String::new()
                    } else {
                        CStr::from_ptr(p).to_string_lossy().trim().to_string()
                    }
                };
                // "CUDA0" → "CUDA".
                let backend = text(sys::ggml_backend_dev_name(device))
                    .trim_end_matches(|c: char| c.is_ascii_digit())
                    .to_string();
                return Device {
                    gpu: true,
                    backend: if backend.is_empty() {
                        "GPU".to_string()
                    } else {
                        backend
                    },
                    name: text(sys::ggml_backend_dev_description(device)),
                };
            }
        }
    }
    Device::cpu()
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
///
/// `gpu` asks for the GPU backend; it has an effect only in a build with one
/// (the CUDA helper), and whisper.cpp falls back to the CPU on its own when
/// the GPU cannot be used.
pub fn transcribe(
    model: &Path,
    samples: &[f32],
    language: Option<&str>,
    gpu: bool,
    progress: &(dyn Fn(f32) + Sync),
    cancel: &AtomicBool,
) -> Result<Transcription, String> {
    // whisper.cpp prints to stderr by default; this routes it nowhere.
    whisper_rs::install_logging_hooks();

    let mut context_params = WhisperContextParameters::default();
    context_params.use_gpu(gpu);
    // Flash attention is whisper.cpp's own default and is faster on the GPU;
    // whisper-rs turns it off. It only conflicts with DTW timestamps, which
    // are not used here (word times come from token timestamps).
    context_params.flash_attn(gpu);
    let context = WhisperContext::new_with_params(model, context_params).map_err(|e| {
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

    // whisper.cpp's timestamps are in centiseconds.
    let micros = |t: i64| t.max(0) * 10_000;
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
        words.push(Word {
            text,
            start: micros(segment.start_timestamp()),
            end: micros(segment.end_timestamp()),
        });
    }
    let language = whisper_rs::get_lang_str(state.full_lang_id_from_state()).map(str::to_string);
    progress(1.0);
    Ok(Transcription { language, words })
}
