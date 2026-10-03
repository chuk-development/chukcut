//! Small shared pieces: file names, time formatting.

use super::*;

pub(crate) fn file_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}

/// `mm:ss:ff` at `fps`, or `mm:ss` when `fps` is zero.
pub(crate) fn timecode(time: Micros, fps: f64) -> String {
    let total = time.max(0) as f64 / 1_000_000.0;
    let minutes = (total / 60.0).floor() as i64;
    let seconds = (total % 60.0).floor() as i64;
    if fps <= 0.0 {
        return format!("{minutes:02}:{seconds:02}");
    }
    let frames = ((total.fract()) * fps).floor() as i64;
    format!("{minutes:02}:{seconds:02}:{frames:02}")
}
