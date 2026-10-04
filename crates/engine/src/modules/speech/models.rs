//! Local Whisper models: which ones, where they live, how they arrive.
//!
//! whisper.cpp's GGML files from the project's own Hugging Face repository,
//! downloaded on first use into the cache directory and checked against a
//! SHA-256 pinned here. The checksums are the repository's Git LFS object ids,
//! which *are* SHA-256 of the file, read from
//! `huggingface.co/api/models/ggerganov/whisper.cpp/tree/main` on 2026-10-03.
//!
//! The cache is the right home: a model is large, derived (it can always be
//! downloaded again) and not the user's work, so "clear cache" may delete it.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

/// The sizes offered. Quantised (`q5_0`) for the two large ones: a third of
/// the download and the memory, for accuracy within a point of the full model.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalModel {
    Tiny,
    #[default]
    Base,
    Small,
    Medium,
    LargeV3Turbo,
}

impl LocalModel {
    pub const ALL: [LocalModel; 5] = [
        LocalModel::Tiny,
        LocalModel::Base,
        LocalModel::Small,
        LocalModel::Medium,
        LocalModel::LargeV3Turbo,
    ];

    pub fn label(self) -> &'static str {
        match self {
            LocalModel::Tiny => "Tiny (75 MB, fastest)",
            LocalModel::Base => "Base (142 MB)",
            LocalModel::Small => "Small (466 MB)",
            LocalModel::Medium => "Medium (515 MB, quantised)",
            LocalModel::LargeV3Turbo => "Large v3 turbo (548 MB, best)",
        }
    }

    pub fn file_name(self) -> &'static str {
        self.spec().0
    }

    /// `(file, bytes, sha256)`.
    fn spec(self) -> (&'static str, u64, &'static str) {
        match self {
            LocalModel::Tiny => (
                "ggml-tiny.bin",
                77_691_713,
                "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21",
            ),
            LocalModel::Base => (
                "ggml-base.bin",
                147_951_465,
                "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe",
            ),
            LocalModel::Small => (
                "ggml-small.bin",
                487_601_967,
                "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
            ),
            LocalModel::Medium => (
                "ggml-medium-q5_0.bin",
                539_212_467,
                "19fea4b380c3a618ec4723c3eef2eb785ffba0d0538cf43f8f235e7b3b34220f",
            ),
            LocalModel::LargeV3Turbo => (
                "ggml-large-v3-turbo-q5_0.bin",
                574_041_195,
                "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2",
            ),
        }
    }

    pub fn size(self) -> u64 {
        self.spec().1
    }

    pub fn sha256(self) -> &'static str {
        self.spec().2
    }

    pub fn url(self) -> String {
        format!(
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/{}",
            self.file_name()
        )
    }

    pub fn path(self) -> PathBuf {
        models_dir().join(self.file_name())
    }

    /// Present at its full size. The checksum is verified once, on download;
    /// re-hashing half a gigabyte at every use would cost seconds.
    pub fn is_downloaded(self) -> bool {
        std::fs::metadata(self.path()).is_ok_and(|m| m.len() == self.size())
    }

    /// The model file, downloading it first if it is not here.
    pub fn ensure(
        self,
        progress: &dyn Fn(u64, u64),
        cancel: &AtomicBool,
    ) -> Result<PathBuf, String> {
        if self.is_downloaded() {
            return Ok(self.path());
        }
        download_verified(
            &self.url(),
            self.sha256(),
            Some(self.size()),
            &self.path(),
            progress,
            cancel,
        )?;
        Ok(self.path())
    }
}

pub fn models_dir() -> PathBuf {
    crate::modules::workspace::paths::cache_root().join("whisper")
}

/// Download `url` to `dest`, refusing anything whose SHA-256 is not `sha256`.
///
/// Streamed to `dest.part` and hashed on the way, then renamed: a cancelled,
/// interrupted or corrupt download never leaves a file at `dest` that a later
/// run would trust because it has the right name.
pub fn download_verified(
    url: &str,
    sha256: &str,
    expected_size: Option<u64>,
    dest: &Path,
    progress: &dyn Fn(u64, u64),
    cancel: &AtomicBool,
) -> Result<(), String> {
    let parent = dest.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)
        .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    let part = dest.with_extension("part");

    let response = crate::modules::cloud::http::agent()
        .get(url)
        .call()
        .map_err(|e| {
            format!(
                "the model download failed: {}",
                crate::modules::cloud::http::describe(e, "")
            )
        })?;
    let total = response
        .header("Content-Length")
        .and_then(|v| v.parse::<u64>().ok())
        .or(expected_size)
        .unwrap_or(0);
    let mut reader = response.into_reader();
    let mut file = std::fs::File::create(&part)
        .map_err(|e| format!("cannot write {}: {e}", part.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 16];
    let mut done = 0u64;
    let mut reported = 0u64;
    let outcome = (|| {
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err("cancelled".to_string());
            }
            let read = reader
                .read(&mut buffer)
                .map_err(|e| format!("the model download broke off: {e}"))?;
            if read == 0 {
                break;
            }
            // A server that sends more than the pinned size is not sending
            // the pinned file; stop before it fills the disk.
            if expected_size.is_some_and(|size| done + read as u64 > size) {
                return Err(format!(
                    "the download is larger than the expected {} bytes",
                    expected_size.unwrap_or(0)
                ));
            }
            hasher.update(&buffer[..read]);
            file.write_all(&buffer[..read])
                .map_err(|e| format!("cannot write {}: {e}", part.display()))?;
            done += read as u64;
            // Every megabyte is plenty for a progress bar and keeps the
            // channel quiet.
            if done - reported >= 1 << 20 {
                reported = done;
                progress(done, total);
            }
        }
        file.sync_all().map_err(|e| e.to_string())?;
        let digest = format!("{:x}", hasher.finalize());
        if !digest.eq_ignore_ascii_case(sha256) {
            return Err(format!(
                "the downloaded model is damaged (checksum {digest}, expected {sha256}); try again"
            ));
        }
        if let Some(size) = expected_size {
            if done != size {
                return Err(format!(
                    "the downloaded model is {done} bytes, expected {size}"
                ));
            }
        }
        Ok(())
    })();
    drop(file);
    match outcome {
        Ok(()) => {
            progress(done, done);
            std::fs::rename(&part, dest)
                .map_err(|e| format!("cannot write {}: {e}", dest.display()))
        }
        Err(error) => {
            let _ = std::fs::remove_file(&part);
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::cloud::http::test_server;

    fn scratch(name: &str) -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-scratch/models");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        let _ = std::fs::remove_file(&path);
        path
    }

    fn sha(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    #[test]
    fn a_download_with_the_right_checksum_lands_and_reports_progress() {
        let blob: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
        let server = test_server::serve(vec![(200, "application/octet-stream", blob.clone())]);
        let dest = scratch("good.bin");
        let seen = std::sync::Mutex::new(Vec::new());
        download_verified(
            &format!("{}/m.bin", server.url),
            &sha(&blob),
            Some(blob.len() as u64),
            &dest,
            &|done, total| seen.lock().unwrap().push((done, total)),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), blob);
        let seen = seen.into_inner().unwrap();
        assert!(seen.len() >= 3, "{seen:?}");
        assert_eq!(*seen.last().unwrap(), (3_000_000, 3_000_000));
    }

    #[test]
    fn a_damaged_download_leaves_nothing_behind() {
        let server = test_server::serve(vec![(
            200,
            "application/octet-stream",
            b"not the model".to_vec(),
        )]);
        let dest = scratch("bad.bin");
        let error = download_verified(
            &format!("{}/m.bin", server.url),
            &sha(b"the real model"),
            None,
            &dest,
            &|_, _| {},
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert!(error.contains("damaged"), "{error}");
        assert!(!dest.exists());
        assert!(!dest.with_extension("part").exists());
    }

    #[test]
    fn a_cancelled_download_leaves_nothing_behind() {
        let server = test_server::serve(vec![(200, "application/octet-stream", vec![0u8; 1000])]);
        let dest = scratch("cancelled.bin");
        let error = download_verified(
            &format!("{}/m.bin", server.url),
            &sha(&[0u8; 1000]),
            None,
            &dest,
            &|_, _| {},
            &AtomicBool::new(true),
        )
        .unwrap_err();
        assert_eq!(error, "cancelled");
        assert!(!dest.exists());
    }

    #[test]
    fn every_model_has_a_checksum_and_a_huggingface_url() {
        for model in LocalModel::ALL {
            assert_eq!(model.sha256().len(), 64);
            assert!(model
                .url()
                .starts_with("https://huggingface.co/ggerganov/whisper.cpp/"));
            assert!(model.size() > 10_000_000);
        }
    }
}
