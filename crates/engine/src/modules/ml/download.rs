//! Downloading models and ONNX Runtime packs into the ML cache.
//!
//! Both are pinned in `chukcut_ml_worker::registry` by URL and SHA-256. A
//! model is one file and goes through the same verified download as the
//! Whisper models (`speech::models::download_verified`): streamed to `.part`,
//! hashed on the way, renamed only when the hash matches.
//!
//! A runtime pack is a `.tgz` of which only the shared libraries are kept.
//! It is unpacked **while** it downloads: the bytes pass through the hasher,
//! then gunzip, then tar, and the libraries land in a staging directory. Only
//! when the last byte has been hashed and matched is the staging directory
//! renamed into place. So the archive (424 MB for the CUDA 12 build) never
//! sits on disk next to what it unpacks to, and a damaged, cut or cancelled
//! download leaves nothing a later run would trust.
//!
//! Requests carry the editor's neutral User-Agent (`chukcut/<version>`) and
//! nothing about the user.

use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use chukcut_ml_worker::registry::{self, Archive, ModelSpec, RuntimePack};
use sha2::{Digest as _, Sha256};

use super::MlError;

/// Progress: bytes done, bytes expected.
pub type Progress<'a> = &'a dyn Fn(u64, u64);

/// The model file for `spec`, downloading it first if it is not here.
///
/// A model whose licence does not allow commercial use is refused unless
/// `allow_noncommercial` is set — see the registry's module docs for why.
pub fn ensure_model(
    root: &Path,
    spec: &ModelSpec,
    allow_noncommercial: bool,
    progress: Progress,
    cancel: &AtomicBool,
) -> Result<PathBuf, MlError> {
    let path = registry::model_path(root, spec);
    if registry::model_present(root, spec) {
        return Ok(path);
    }
    if !spec.commercial_ok && !allow_noncommercial {
        return Err(MlError::Failed(format!(
            "{} is licensed {} and may not be used in a paid feature; it is only downloaded on request",
            spec.name, spec.licence
        )));
    }
    crate::modules::speech::models::download_verified(
        spec.url,
        spec.sha256,
        Some(spec.bytes),
        &path,
        progress,
        cancel,
    )
    .map_err(|e| {
        if cancel.load(Ordering::Relaxed) {
            MlError::Cancelled
        } else {
            MlError::Failed(format!("could not download {}: {e}", spec.name))
        }
    })?;
    Ok(path)
}

/// Unpack `pack` into the ML cache, downloading it, unless it is there.
pub fn ensure_runtime(
    root: &Path,
    pack: &RuntimePack,
    progress: Progress,
    cancel: &AtomicBool,
) -> Result<PathBuf, MlError> {
    let library = registry::runtime_library(root, pack);
    if registry::runtime_present(root, pack) {
        return Ok(library);
    }
    if pack.archive == Archive::Wheel {
        install_wheel(root, pack, progress, cancel)?;
        return Ok(library);
    }
    let response = crate::modules::cloud::http::agent()
        .get(pack.url)
        .call()
        .map_err(|e| {
            MlError::Failed(format!(
                "could not download {}: {}",
                pack.name,
                crate::modules::cloud::http::describe(e, "")
            ))
        })?;
    unpack_runtime(root, pack, response.into_reader(), progress, cancel)?;
    Ok(library)
}

/// A wheel is a zip, whose directory is at its end: it is downloaded
/// (verified) to a file beside its destination, the libraries are taken out
/// into a staging directory, and the file is deleted.
fn install_wheel(
    root: &Path,
    pack: &RuntimePack,
    progress: Progress,
    cancel: &AtomicBool,
) -> Result<(), MlError> {
    let dir = registry::runtime_dir(root, pack);
    let wheel = dir.with_extension("whl");
    let failed = |e: String| {
        if cancel.load(Ordering::Relaxed) {
            MlError::Cancelled
        } else {
            MlError::Failed(format!("could not install {}: {e}", pack.name))
        }
    };
    crate::modules::speech::models::download_verified(
        pack.url,
        pack.sha256,
        Some(pack.bytes),
        &wheel,
        progress,
        cancel,
    )
    .map_err(|e| failed(e.to_string()))?;
    let staging = dir.with_extension("part");
    let _ = std::fs::remove_dir_all(&staging);
    let result = extract_wheel(pack, &wheel, &staging.join("lib"));
    let _ = std::fs::remove_file(&wheel);
    match result {
        Ok(kept) if staging.join("lib").join(pack.library).exists() => {
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::rename(&staging, &dir)
                .map_err(|e| MlError::Failed(format!("cannot write {}: {e}", dir.display())))?;
            tracing::info!(
                "{} unpacked: {kept} libraries in {}",
                pack.name,
                dir.display()
            );
            Ok(())
        }
        Ok(_) => {
            let _ = std::fs::remove_dir_all(&staging);
            Err(failed(format!("it holds no {}", pack.library)))
        }
        Err(e) => {
            let _ = std::fs::remove_dir_all(&staging);
            Err(failed(e))
        }
    }
}

/// The libraries of `wheel` into `lib`, flattened to their file names so an
/// entry can never write outside it.
fn extract_wheel(pack: &RuntimePack, wheel: &Path, lib: &Path) -> Result<usize, String> {
    std::fs::create_dir_all(lib).map_err(|e| e.to_string())?;
    let file = std::fs::File::open(wheel).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    let mut kept = 0;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        let path = entry.name().to_string();
        let Some(name) = registry::keep_from_runtime_archive(pack, &path) else {
            continue;
        };
        let target = lib.join(name);
        let mut out = std::fs::File::create(&target).map_err(|e| e.to_string())?;
        io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755));
        kept += 1;
    }
    Ok(kept)
}

/// Hashes and counts what passes through, and stops on cancel.
struct Tee<'a, R> {
    inner: R,
    hasher: Sha256,
    done: u64,
    reported: u64,
    total: u64,
    progress: Progress<'a>,
    cancel: &'a AtomicBool,
}

impl<R: Read> Read for Tee<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.cancel.load(Ordering::Relaxed) {
            // Not `Interrupted`: readers retry that kind forever.
            return Err(io::Error::other("cancelled"));
        }
        let n = self.inner.read(buf)?;
        self.hasher.update(&buf[..n]);
        self.done += n as u64;
        if self.done - self.reported >= 1 << 20 {
            self.reported = self.done;
            (self.progress)(self.done, self.total);
        }
        Ok(n)
    }
}

/// The verified-unpack step of [`ensure_runtime`], separate so it can be
/// tested on an archive built in memory.
pub fn unpack_runtime(
    root: &Path,
    pack: &RuntimePack,
    body: impl Read,
    progress: Progress,
    cancel: &AtomicBool,
) -> Result<(), MlError> {
    let dir = registry::runtime_dir(root, pack);
    let staging = dir.with_extension("part");
    let _ = std::fs::remove_dir_all(&staging);
    let lib = staging.join("lib");
    std::fs::create_dir_all(&lib)
        .map_err(|e| MlError::Failed(format!("cannot create {}: {e}", lib.display())))?;
    let mut tee = Tee {
        inner: body,
        hasher: Sha256::new(),
        done: 0,
        reported: 0,
        total: pack.bytes,
        progress,
        cancel,
    };
    let outcome = (|| -> Result<usize, String> {
        let mut kept = 0usize;
        {
            let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(&mut tee));
            for entry in archive.entries().map_err(|e| e.to_string())? {
                let mut entry = entry.map_err(|e| e.to_string())?;
                let path = entry
                    .path()
                    .map_err(|e| e.to_string())?
                    .to_string_lossy()
                    .into_owned();
                let Some(name) = registry::keep_from_runtime_archive(pack, &path) else {
                    continue;
                };
                // Flattened into `lib/`, by file name only: an entry can
                // never write outside the staging directory.
                let target = lib.join(name);
                match entry.header().entry_type() {
                    tar::EntryType::Symlink => {
                        let link = entry
                            .link_name()
                            .map_err(|e| e.to_string())?
                            .ok_or("a symlink without a target")?;
                        let link = link
                            .file_name()
                            .ok_or("a symlink to a directory")?
                            .to_owned();
                        std::os::unix::fs::symlink(link, &target).map_err(|e| e.to_string())?;
                    }
                    tar::EntryType::Regular => {
                        let mut file = std::fs::File::create(&target).map_err(|e| e.to_string())?;
                        io::copy(&mut entry, &mut file).map_err(|e| e.to_string())?;
                        use std::os::unix::fs::PermissionsExt as _;
                        let _ = std::fs::set_permissions(
                            &target,
                            std::fs::Permissions::from_mode(0o755),
                        );
                    }
                    _ => continue,
                }
                kept += 1;
            }
        }
        // Whatever follows the tar's end (gzip padding) is part of the file
        // the checksum covers.
        io::copy(&mut tee, &mut io::sink()).map_err(|e| e.to_string())?;
        let digest = format!("{:x}", tee.hasher.clone().finalize());
        if !digest.eq_ignore_ascii_case(pack.sha256) {
            return Err(format!(
                "the download is damaged (checksum {digest}, expected {}); try again",
                pack.sha256
            ));
        }
        Ok(kept)
    })();
    match outcome {
        Ok(kept) if staging.join("lib").join(pack.library).exists() => {
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::rename(&staging, &dir)
                .map_err(|e| MlError::Failed(format!("cannot write {}: {e}", dir.display())))?;
            progress(tee.done, tee.done);
            tracing::info!(
                "{} unpacked: {kept} libraries in {}",
                pack.name,
                dir.display()
            );
            Ok(())
        }
        Ok(_) => {
            let _ = std::fs::remove_dir_all(&staging);
            Err(MlError::Failed(format!(
                "{} holds no libonnxruntime.so",
                pack.name
            )))
        }
        Err(e) => {
            let _ = std::fs::remove_dir_all(&staging);
            if cancel.load(Ordering::Relaxed) {
                Err(MlError::Cancelled)
            } else {
                Err(MlError::Failed(format!(
                    "could not install {}: {e}",
                    pack.name
                )))
            }
        }
    }
}

/// Bytes a directory holds on disk, for the settings page.
pub fn size_of(path: &Path) -> u64 {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if meta.is_dir() {
        std::fs::read_dir(path)
            .map(|entries| entries.flatten().map(|e| size_of(&e.path())).sum())
            .unwrap_or(0)
    } else if meta.is_file() {
        meta.len()
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-scratch/ml")
            .join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A `.tgz` shaped like Microsoft's: a top directory, `lib/` with the
    /// library, a symlink chain to it, and files we do not keep.
    fn archive() -> Vec<u8> {
        let mut tar = tar::Builder::new(Vec::new());
        let mut add = |path: &str, data: &[u8]| {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, path, data).unwrap();
        };
        add("ort-x/lib/libonnxruntime.so.1.28.3", b"ELF pretend");
        add(
            "ort-x/lib/libonnxruntime_providers_tensorrt.so",
            b"too big to keep",
        );
        add("ort-x/include/onnxruntime_c_api.h", b"/* header */");
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        header.set_cksum();
        tar.append_link(
            &mut header,
            "ort-x/lib/libonnxruntime.so",
            "libonnxruntime.so.1.28.3",
        )
        .unwrap();
        let tar = tar.into_inner().unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        io::Write::write_all(&mut gz, &tar).unwrap();
        gz.finish().unwrap()
    }

    fn pack_for(bytes: &[u8]) -> RuntimePack {
        let sha = format!("{:x}", Sha256::digest(bytes));
        RuntimePack {
            sha256: Box::leak(sha.into_boxed_str()),
            bytes: bytes.len() as u64,
            ..*registry::runtime_pack("cpu").unwrap()
        }
    }

    #[test]
    fn a_runtime_pack_unpacks_only_its_libraries_and_only_when_verified() {
        let root = scratch("unpack");
        let bytes = archive();
        let pack = pack_for(&bytes);
        let reported = std::sync::Mutex::new(Vec::new());
        unpack_runtime(
            &root,
            &pack,
            bytes.as_slice(),
            &|d, t| reported.lock().unwrap().push((d, t)),
            &AtomicBool::new(false),
        )
        .unwrap();
        let lib = registry::runtime_dir(&root, &pack).join("lib");
        let mut names: Vec<String> = std::fs::read_dir(&lib)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["libonnxruntime.so", "libonnxruntime.so.1.28.3"]);
        assert_eq!(
            std::fs::read(lib.join("libonnxruntime.so")).unwrap(),
            b"ELF pretend"
        );
        assert!(registry::runtime_present(&root, &pack));
        assert_eq!(
            reported.lock().unwrap().last(),
            Some(&(bytes.len() as u64, bytes.len() as u64))
        );
    }

    #[test]
    fn a_damaged_runtime_pack_leaves_nothing_behind() {
        let root = scratch("damaged");
        let bytes = archive();
        let mut pack = pack_for(&bytes);
        pack.sha256 = "0000000000000000000000000000000000000000000000000000000000000000";
        let error = unpack_runtime(
            &root,
            &pack,
            bytes.as_slice(),
            &|_, _| {},
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert!(error.to_string().contains("damaged"), "{error}");
        assert!(!registry::runtime_dir(&root, &pack).exists());
        assert!(!registry::runtime_dir(&root, &pack)
            .with_extension("part")
            .exists());
    }

    #[test]
    fn a_cancelled_runtime_download_says_so_and_leaves_nothing() {
        let root = scratch("cancelled");
        let bytes = archive();
        let pack = pack_for(&bytes);
        let error = unpack_runtime(
            &root,
            &pack,
            bytes.as_slice(),
            &|_, _| {},
            &AtomicBool::new(true),
        )
        .unwrap_err();
        assert_eq!(error, MlError::Cancelled);
        assert!(!registry::runtime_dir(&root, &pack).exists());
    }

    #[test]
    fn a_noncommercial_model_is_not_fetched_by_accident() {
        let root = scratch("licence");
        let spec = ModelSpec {
            commercial_ok: false,
            licence: "CC-BY-NC-4.0",
            // A URL that would fail if it were ever fetched.
            url: "https://invalid.invalid/model.onnx",
            ..*registry::model("yunet").unwrap()
        };
        let error =
            ensure_model(&root, &spec, false, &|_, _| {}, &AtomicBool::new(false)).unwrap_err();
        assert!(error.to_string().contains("CC-BY-NC-4.0"), "{error}");
    }
}
