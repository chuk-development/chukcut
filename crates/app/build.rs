//! Hands the build's git commit to the startup block of the log
//! (`chukcut_engine::modules::diag::startup`), as `CHUKCUT_GIT_COMMIT`.
//!
//! In the app crate rather than the engine because a commit reruns this
//! script, and rebuilding the app is minutes cheaper than rebuilding the
//! engine. A build outside a git checkout (a source tarball) says `unknown`.

use std::path::Path;
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

fn main() {
    let commit = git(&["rev-parse", "--short=10", "HEAD"]).unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=CHUKCUT_GIT_COMMIT={commit}");

    // Run again when HEAD moves: HEAD itself (a checkout), the branch it
    // names (a commit) and packed-refs (a gc). Only files that exist are
    // named, because cargo reruns a script on every build for a missing one.
    let mut watched = vec!["HEAD".to_string(), "packed-refs".to_string()];
    if let Some(branch) = git(&["symbolic-ref", "-q", "HEAD"]) {
        watched.push(branch);
    }
    for name in watched {
        if let Some(path) = git(&["rev-parse", "--git-path", &name]) {
            if Path::new(&path).exists() {
                println!("cargo:rerun-if-changed={path}");
            }
        }
    }
}
