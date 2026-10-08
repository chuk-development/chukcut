//! FFmpeg's own complaints, in the log file.
//!
//! FFmpeg prints through `av_log`, which goes to stderr: a decoder that
//! conceals a broken slice or a muxer that refuses a timestamp says so on a
//! terminal nobody looks at, and the log file never knew. [`install`] routes
//! every message FFmpeg would print (`media::ensure_initialized` sets the
//! level to errors only) into the log as well, tagged with the FFmpeg
//! component that raised it:
//!
//! ```text
//! WARN chukcut_engine::modules::diag::ffmpeg_log: ffmpeg: [h264 @ 0x7f3a2c01b600] error while decoding MB 54 31
//! ```
//!
//! It still goes to stderr too. A corrupt file can make a decoder complain on
//! every frame, so the log takes at most [`LINES_PER_MINUTE`] of these a
//! minute and counts the rest.
//!
//! x86-64 only for now: the callback receives a C `va_list`, whose Rust type
//! differs per architecture. Elsewhere [`install`] does nothing and FFmpeg
//! keeps printing to stderr only.

/// FFmpeg lines the log takes per minute.
pub const LINES_PER_MINUTE: u32 = 60;

/// Install the callback. Once per process; later calls do nothing.
pub fn install() {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    imp::install();
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod imp {
    use std::cell::Cell;
    use std::ffi::{c_char, c_int, c_void, CStr};
    use std::time::{Duration, Instant};

    use ffmpeg_next::ffi;
    use parking_lot::Mutex;

    use super::super::child::{Admit, LineGate};
    use super::LINES_PER_MINUTE;

    static GATE: Mutex<Option<LineGate>> = parking_lot::const_mutex(None);

    thread_local! {
        /// `av_log_format_line2`'s "does the next fragment start a line"
        /// state, which the default callback keeps in a static. Per thread
        /// here, because FFmpeg logs from its decoder threads at once.
        static PRINT_PREFIX: Cell<c_int> = const { Cell::new(1) };
    }

    pub fn install() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            *GATE.lock() = Some(LineGate::new(LINES_PER_MINUTE, Duration::from_secs(60)));
            // SAFETY: the callback has the signature `av_log_set_callback`
            // wants and never unwinds into C.
            unsafe { ffi::av_log_set_callback(Some(callback)) };
        });
    }

    unsafe extern "C" fn callback(
        avcl: *mut c_void,
        level: c_int,
        fmt: *const c_char,
        vl: *mut ffi::__va_list_tag,
    ) {
        // SAFETY: reading FFmpeg's global level.
        if level > unsafe { ffi::av_log_get_level() } {
            return;
        }
        let mut buf = [0 as c_char; 1024];
        let mut prefix = PRINT_PREFIX.with(Cell::get);
        // SAFETY: the arguments are the ones FFmpeg handed us, consumed once;
        // `buf` is large enough and always NUL-terminated by FFmpeg.
        unsafe {
            ffi::av_log_format_line2(
                avcl,
                level,
                fmt,
                vl,
                buf.as_mut_ptr(),
                buf.len() as c_int,
                &mut prefix,
            );
        }
        PRINT_PREFIX.with(|cell| cell.set(prefix));
        // SAFETY: NUL-terminated by `av_log_format_line2`.
        let text = unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy();
        // The va_list is consumed, so FFmpeg's default callback cannot print
        // it; print the formatted text where it would have gone.
        eprint!("{text}");
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        // Unwinding into C is undefined behaviour; a log line is not worth it.
        let _ = std::panic::catch_unwind(|| forward(level, text));
    }

    fn forward(level: c_int, text: &str) {
        let admit = match GATE.lock().as_mut() {
            Some(gate) => gate.admit(Instant::now()),
            None => Admit::Yes,
        };
        let dropped = match admit {
            Admit::No => return,
            Admit::Yes => 0,
            Admit::YesAfterDropping(dropped) => dropped,
        };
        if dropped > 0 {
            tracing::warn!(
                "ffmpeg: dropped {dropped} messages over the limit of {LINES_PER_MINUTE} a minute"
            );
        }
        // Never ERROR, not even for FFmpeg's FATAL: every FFmpeg call has a
        // caller that handles the failure and says what it meant, and a
        // probe that expects to fail (NVENC's trial of AV1 on a card without
        // it says "No capable devices found" at FATAL) is not an error.
        if level <= ffi::AV_LOG_WARNING {
            tracing::warn!("ffmpeg: {text}");
        } else {
            tracing::info!("ffmpeg: {text}");
        }
    }
}
