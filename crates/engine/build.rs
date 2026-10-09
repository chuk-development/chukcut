//! Builds `chukcut-whisper-cuda`, the CUDA transcription helper, next to the
//! binaries when this machine can build it.
//!
//! The helper is whisper.cpp with its CUDA backend in a process of its own
//! (`crates/whisper`, decision 0036). It has to be built with `nvcc`, and a
//! cargo feature cannot depend on whether a compiler is installed, so this
//! script decides at build time: with the `local-whisper` feature (the app
//! and the CLI turn it on), on Linux, when `nvcc` is found, it runs a second
//! `cargo build` of the helper into `<target>/whisper-cuda/` and copies the
//! binary into the profile directory beside `chukcut`, where the engine looks
//! for it. Without `nvcc` (CI, the release builders) it does nothing, and the
//! editor transcribes on the CPU.
//!
//! `CHUKCUT_WHISPER_CUDA=0` skips the helper; `=1` requires it and fails the
//! build when it cannot be built. The first build compiles ggml's CUDA
//! kernels and takes several minutes; afterwards the nested cargo is a no-op.
//! A failed build is reported as a warning once and not retried until the
//! helper's sources or this variable change.

use std::env;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

const HELPER: &str = "chukcut-whisper-cuda";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=CHUKCUT_WHISPER_CUDA");
    // Set by `cargo clippy`; a lint run does not need the helper.
    println!("cargo:rerun-if-env-changed=RUSTC_WORKSPACE_WRAPPER");
    println!("cargo:rerun-if-env-changed=CUDACXX");

    if env::var_os("CARGO_FEATURE_LOCAL_WHISPER").is_none() {
        return;
    }
    let wanted = env::var("CHUKCUT_WHISPER_CUDA").unwrap_or_default();
    let required = matches!(wanted.as_str(), "1" | "on" | "yes");
    if matches!(wanted.as_str(), "0" | "off" | "no") {
        return;
    }
    let skip = |why: &str| {
        if required {
            panic!("CHUKCUT_WHISPER_CUDA=1, but the CUDA whisper helper cannot be built: {why}");
        }
    };
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        return skip("it is built on Linux only");
    }
    if env::var("TARGET").ok() != env::var("HOST").ok() {
        return skip("this is a cross build");
    }
    if !required && env::var("RUSTC_WORKSPACE_WRAPPER").is_ok_and(|w| w.contains("clippy")) {
        return;
    }
    let Some(nvcc) = find_nvcc() else {
        return skip("nvcc is not on PATH (install the CUDA toolkit)");
    };

    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("set by cargo"));
    let Some(root) = manifest.parent().and_then(Path::parent) else {
        return skip("the workspace root is not where it should be");
    };
    let crate_dir = root.join("crates").join("whisper");
    if !crate_dir.join("Cargo.toml").is_file() {
        // The engine built outside this workspace: no helper sources.
        return skip("crates/whisper is not next to the engine");
    }
    // OUT_DIR is <target>/<profile>/build/<package>-<hash>/out.
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("set by cargo"));
    let Some(profile_dir) = out_dir.ancestors().nth(3).map(Path::to_path_buf) else {
        return skip("OUT_DIR has an unexpected shape");
    };
    let Some(target_dir) = profile_dir.parent() else {
        return skip("OUT_DIR has an unexpected shape");
    };
    // One helper build for the debug and the release profile alike: the
    // helper is always built in release (whisper.cpp is Release either way,
    // and its Rust part is a hundred lines).
    let nested = target_dir.join("whisper-cuda");

    println!("cargo:rerun-if-changed={}", crate_dir.display());
    println!(
        "cargo:rerun-if-changed={}",
        root.join("Cargo.lock").display()
    );

    let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = Command::new(cargo);
    command
        .current_dir(root)
        .args(["build", "--release", "--locked", "--offline"])
        .args([
            "-p",
            "chukcut-whisper",
            "--features",
            "cuda",
            "--bin",
            HELPER,
        ])
        .arg("--target-dir")
        .arg(&nested)
        .env("CUDACXX", &nvcc);
    // The outer build's variables describe the outer build (its target dir,
    // its rustflags, its package); the nested cargo must not read them as
    // its own configuration. The jobserver (CARGO_MAKEFLAGS) stays, so the
    // nested build shares the outer `-j` instead of adding to it.
    for (key, _) in env::vars_os() {
        let Some(key) = key.to_str() else { continue };
        let outer = (key.starts_with("CARGO_") && key != "CARGO_HOME" && key != "CARGO_MAKEFLAGS")
            || matches!(
                key,
                "RUSTC_WORKSPACE_WRAPPER"
                    | "OUT_DIR"
                    | "TARGET"
                    | "HOST"
                    | "PROFILE"
                    | "OPT_LEVEL"
                    | "DEBUG"
                    | "NUM_JOBS"
                    | "RUSTC_LINKER"
            );
        if outer {
            command.env_remove(key);
        }
    }

    let output = match command.output() {
        Ok(output) => output,
        Err(e) => return fail(required, &format!("cargo could not be started: {e}"), ""),
    };
    if !output.status.success() {
        let log = String::from_utf8_lossy(&output.stderr);
        return fail(required, "the nested cargo build failed", &log);
    }

    let built = nested.join("release").join(HELPER);
    let destination = profile_dir.join(HELPER);
    if let Err(e) = install(&built, &destination) {
        return fail(
            required,
            &format!("{} could not be copied: {e}", built.display()),
            "",
        );
    }
    // A deleted helper is built (copied) again on the next build.
    println!("cargo:rerun-if-changed={}", destination.display());
}

/// `CUDACXX` when it names a file, else `nvcc` on PATH, else the CUDA
/// toolkit's usual places.
fn find_nvcc() -> Option<PathBuf> {
    if let Some(path) = env::var_os("CUDACXX").map(PathBuf::from) {
        return path.is_file().then_some(path);
    }
    let on_path = env::var_os("PATH").and_then(|path| {
        env::split_paths(&path)
            .map(|dir| dir.join("nvcc"))
            .find(|p| p.is_file())
    });
    on_path.or_else(|| {
        ["/usr/local/cuda/bin/nvcc", "/opt/cuda/bin/nvcc"]
            .iter()
            .map(PathBuf::from)
            .find(|p| p.is_file())
    })
}

/// Copy `from` to `to` through a temporary name, so a helper that is running
/// is replaced rather than written into ("text file busy"), and give the copy
/// the source's modification time, so cargo's `rerun-if-changed` on it does
/// not fire on every build.
fn install(from: &Path, to: &Path) -> std::io::Result<()> {
    let source = std::fs::metadata(from)?;
    if let Ok(existing) = std::fs::metadata(to) {
        if existing.len() == source.len() && existing.modified()? == source.modified()? {
            return Ok(());
        }
    }
    let temporary = to.with_extension(OsStr::new("new"));
    std::fs::copy(from, &temporary)?;
    std::fs::File::options()
        .write(true)
        .open(&temporary)?
        .set_modified(source.modified()?)?;
    std::fs::rename(&temporary, to)
}

fn fail(required: bool, what: &str, log: &str) {
    // The last lines of cargo's output are the ones that say why.
    let tail: Vec<&str> = log.lines().rev().take(25).collect();
    if required {
        panic!(
            "the CUDA whisper helper could not be built: {what}\n{}",
            tail.into_iter().rev().collect::<Vec<_>>().join("\n")
        );
    }
    println!(
        "cargo:warning=the CUDA whisper helper was not built ({what}); transcription runs on the CPU. CHUKCUT_WHISPER_CUDA=0 silences this, =1 makes it an error."
    );
    for line in tail.into_iter().rev() {
        println!("cargo:warning=  {line}");
    }
}
