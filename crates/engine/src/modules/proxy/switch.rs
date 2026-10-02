//! Which file a consumer opens — and the type that stops the wrong one.
//!
//! There is exactly one catastrophic bug in a proxy feature and it is not
//! subtle in its consequences, only in its symptoms: **a proxy reaching the
//! export**. Nothing fails. No warning appears. The render finishes at the
//! usual speed and produces a file that decodes, plays, and is 720p. It is
//! discovered by the viewer, after the upload.
//!
//! Comments do not prevent that. A boolean flag threaded through six call sites
//! does not prevent it either — it prevents it until somebody adds a seventh.
//! So this file makes it a type error:
//!
//! - [`PreviewSource`] is the **only** type in the codebase that can hold a
//!   proxy path. It is what `media::provider` is built from when the provider
//!   is serving the preview.
//! - [`ExportSource`] is what the export path is built from. It has no field a
//!   proxy could live in, and there is deliberately no `From<PreviewSource>`,
//!   no `into_export`, and no accessor on `PreviewSource` that hands out a
//!   `PathBuf` an export could accept. Turning one into the other requires
//!   writing `ExportSource::deliverable(...)` with a path you got from
//!   somewhere else, which is a line a reviewer will see.
//! - And because "somewhere else" could still be a proxy — a path read back out
//!   of the cache index, say — [`ExportSource::deliverable`] **refuses any path
//!   underneath `paths::proxies_dir()`** and returns
//!   [`ProxyError::ProxyInDeliverable`]. That is the belt under the braces, and
//!   unlike the type-level half it can be, and is, tested at runtime.
//!
//! The asymmetry is deliberate. `PreviewSource::original()` exists, because the
//! preview genuinely needs the original: audio is decoded from it (the proxy
//! carries no audio track at all), and the media panel shows its real
//! resolution. `ExportSource` has no route back to a proxy in either direction.

use std::path::{Path, PathBuf};

use crate::modules::workspace::paths;

use super::{ProxyError, Result};

/// Whether `path` lives inside the proxy cache.
///
/// A prefix comparison on the canonical form of both, falling back to the
/// literal paths when either cannot be canonicalised — a proxy that has just
/// been deleted, or a source on a disconnected volume, must still be *judged*,
/// and judged conservatively.
pub fn is_inside_proxy_cache(path: &Path) -> bool {
    inside(path, &paths::proxies_dir())
}

fn inside(path: &Path, root: &Path) -> bool {
    let canonical_root = root.canonicalize();
    let canonical_path = path.canonicalize();
    match (&canonical_path, &canonical_root) {
        (Ok(p), Ok(r)) => p.starts_with(r),
        // Either side is missing from the filesystem. Compare what we were
        // given, which still catches the case that matters: a path handed
        // around inside the process that was built from `proxies_dir()`.
        _ => path.starts_with(root),
    }
}

/// A source as the **preview** sees it: the original, plus the proxy standing
/// in for it when one exists.
///
/// Constructed by the provider from a project snapshot and a cache lookup. The
/// only way to reach the proxy's path is [`Self::decode_path`], which is named
/// so that a caller cannot pretend not to have known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewSource {
    original: PathBuf,
    proxy: Option<PathBuf>,
}

impl PreviewSource {
    /// A source with no proxy — the state everything starts in, and the state
    /// it stays in for footage the rule says is fine as it is.
    pub fn original_only(original: impl Into<PathBuf>) -> Self {
        Self {
            original: original.into(),
            proxy: None,
        }
    }

    /// A source with a proxy standing in for it.
    ///
    /// `proxy` is trusted to exist; the cache checks that before handing one
    /// out, and re-`stat`ing it here would put a syscall on the render path.
    pub fn with_proxy(original: impl Into<PathBuf>, proxy: impl Into<PathBuf>) -> Self {
        Self {
            original: original.into(),
            proxy: Some(proxy.into()),
        }
    }

    /// The file a decoder should open for **pixels**: the proxy when there is
    /// one, the original otherwise.
    pub fn decode_path(&self) -> &Path {
        self.proxy.as_deref().unwrap_or(&self.original)
    }

    /// The file the audio and the media panel read.
    ///
    /// Always the original: a proxy carries no audio track, and the panel shows
    /// what the user actually imported.
    pub fn original(&self) -> &Path {
        &self.original
    }

    pub fn is_proxied(&self) -> bool {
        self.proxy.is_some()
    }
}

/// A source as the **export** sees it. Always an original, by construction.
///
/// There is no constructor, conversion or accessor anywhere in this crate that
/// puts a proxy inside one of these. See the file's documentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportSource {
    original: PathBuf,
}

impl ExportSource {
    /// Accept `path` as something safe to put in a deliverable.
    ///
    /// Errors when it is a proxy. The check is cheap — one prefix comparison —
    /// and it runs once per material at export setup, not per frame.
    pub fn deliverable(path: impl Into<PathBuf>) -> Result<Self> {
        let original = path.into();
        if is_inside_proxy_cache(&original) {
            return Err(ProxyError::ProxyInDeliverable(original));
        }
        Ok(Self { original })
    }

    /// The file the encoder reads. There is only ever one answer.
    pub fn path(&self) -> &Path {
        &self.original
    }
}

impl From<&PreviewSource> for PathBuf {
    /// Deliberately the **original**, not `decode_path`.
    ///
    /// Somebody will eventually reach for a `PathBuf` from a `PreviewSource`
    /// without reading either method's name. When they do, the value they get
    /// is the safe one, and the preview keeps working because the preview goes
    /// through [`PreviewSource::decode_path`] explicitly.
    fn from(source: &PreviewSource) -> Self {
        source.original.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_source_without_a_proxy_decodes_from_the_original() {
        let source = PreviewSource::original_only("/footage/a.mp4");
        assert_eq!(source.decode_path(), Path::new("/footage/a.mp4"));
        assert_eq!(source.original(), Path::new("/footage/a.mp4"));
        assert!(!source.is_proxied());
    }

    #[test]
    fn a_proxied_source_decodes_from_the_proxy_and_keeps_the_original() {
        let source = PreviewSource::with_proxy("/footage/a.mp4", "/cache/a-proxy.mp4");
        assert_eq!(source.decode_path(), Path::new("/cache/a-proxy.mp4"));
        // Audio, and the media panel, still see the real file.
        assert_eq!(source.original(), Path::new("/footage/a.mp4"));
        assert!(source.is_proxied());
    }

    #[test]
    fn the_lazy_conversion_out_of_a_preview_source_yields_the_original() {
        let source = PreviewSource::with_proxy("/footage/a.mp4", "/cache/a-proxy.mp4");
        let path: PathBuf = (&source).into();
        assert_eq!(path, PathBuf::from("/footage/a.mp4"));
    }

    #[test]
    fn an_ordinary_file_is_deliverable() {
        let export = ExportSource::deliverable("/footage/a.mp4").expect("not a proxy");
        assert_eq!(export.path(), Path::new("/footage/a.mp4"));
    }

    /// The load-bearing test. If this ever fails, the editor can ship a 720p
    /// master and say nothing about it.
    #[test]
    fn a_path_inside_the_proxy_cache_is_refused_as_a_deliverable() {
        let proxy = paths::proxies_dir().join("clip-0123456789abcdef.mp4");
        let error = ExportSource::deliverable(&proxy).expect_err("a proxy must be refused");
        assert!(matches!(error, ProxyError::ProxyInDeliverable(_)));
        // The message has to name the file, because the person reading it is
        // being told their export was stopped.
        assert!(error.to_string().contains("clip-0123456789abcdef.mp4"));
    }

    #[test]
    fn nested_proxy_paths_are_refused_too() {
        let nested = paths::proxies_dir().join("sub").join("dir").join("x.mp4");
        assert!(is_inside_proxy_cache(&nested));
        assert!(ExportSource::deliverable(&nested).is_err());
    }

    #[test]
    fn a_path_merely_mentioning_proxies_is_not_a_proxy() {
        // The check is a path-prefix one, not a substring one: a user folder
        // called `proxies` must not become un-exportable.
        for path in [
            "/home/user/proxies/clip.mp4",
            "/home/user/my proxies/clip.mp4",
            "/footage/proxy-shoot/clip.mp4",
        ] {
            assert!(!is_inside_proxy_cache(Path::new(path)), "{path}");
            assert!(ExportSource::deliverable(path).is_ok(), "{path}");
        }
    }

    /// Not a test of behaviour but of *shape*, and it is the reason the whole
    /// file exists. Kept as a test so that deleting the property is a red test
    /// rather than a silent regression.
    #[test]
    fn there_is_no_route_from_a_preview_source_into_an_export_source() {
        let source = PreviewSource::with_proxy("/footage/a.mp4", "/cache/a-proxy.mp4");

        // The only `PathBuf` a `PreviewSource` will give up is the original, so
        // even the sloppiest possible conversion is safe.
        let export = ExportSource::deliverable(PathBuf::from(&source)).expect("original");
        assert_eq!(export.path(), source.original());
        assert_ne!(export.path(), source.decode_path());

        // And `decode_path` is refused by name, if it ever came from the cache.
        // (Here it is a literal, so it is accepted — the runtime guard is
        // tested against a real cache path above. What this asserts is that the
        // two accessors are genuinely different values.)
        assert_ne!(source.decode_path(), source.original());
    }
}
