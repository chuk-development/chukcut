//! Writing a file so that nobody ever reads half of it, and so that two
//! writers of the same file cannot break each other.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

/// Write `bytes` to `path` through a partial file beside it, then rename.
///
/// The partial file's name is unique per write — process id and a counter —
/// not just `<name>.part`. Two threads that install the same file at once (a
/// preview tile and a new project both making sure the LUT library is there,
/// or two tests in one binary) would otherwise share one partial file: the
/// second `write` truncates what the first is about to rename, and the
/// second `rename` then fails because the first one already moved the file
/// away. That was "cannot write …/luts/Moody Green.cube" in
/// `tests/templates.rs`. With a name of its own, each writer renames a
/// complete file over the target, and the last rename wins with the same
/// bytes.
pub fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), String> {
    static WRITES: AtomicU64 = AtomicU64::new(0);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    // The name still ends in `.part`, which is what the cache trimmer skips.
    let partial = path.with_extension(format!(
        "{}.{}-{}.part",
        path.extension().and_then(|e| e.to_str()).unwrap_or("tmp"),
        std::process::id(),
        WRITES.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&partial, bytes).map_err(|e| {
        let _ = std::fs::remove_file(&partial);
        format!("cannot write {}: {e}", partial.display())
    })?;
    std::fs::rename(&partial, path).map_err(|e| {
        let _ = std::fs::remove_file(&partial);
        format!("cannot write {}: {e}", path.display())
    })
}

/// [`write_atomically`] for a PNG: encoded in memory, then written the same
/// way. Cached tiles and previews are drawn on demand by whoever asks first,
/// and two askers at once is ordinary.
pub fn save_png_atomically(path: &Path, image: &image::RgbaImage) -> Result<(), String> {
    let mut bytes = Vec::new();
    image
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .map_err(|e| format!("encoding {} failed: {e}", path.display()))?;
    write_atomically(path, &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn many_writers_of_one_file_all_succeed_and_leave_no_partials() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-scratch/workspace")
            .join(format!("atomic-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let target = dir.join("Moody Green.cube");
        let body = "x".repeat(256 * 1024);
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    for _ in 0..20 {
                        write_atomically(&target, body.as_bytes()).unwrap();
                    }
                });
            }
        });
        assert_eq!(std::fs::read_to_string(&target).unwrap(), body);
        let names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("Moody Green.cube")]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
