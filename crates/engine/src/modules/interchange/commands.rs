//! The shell-facing API of timeline interchange.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;

use super::{edl, fcpxml, otio, timeline, Format, Report};
use crate::state::AppState;

/// What an export wrote, and what it had to leave behind.
#[derive(Debug, Clone, Serialize)]
pub struct InterchangeExport {
    pub path: PathBuf,
    pub format: Format,
    /// Clips written, over all tracks.
    pub clips: usize,
    pub warnings: Vec<String>,
}

/// The formats and their file extensions, for a save dialog.
pub fn interchange_formats() -> Vec<(Format, &'static str, &'static str)> {
    Format::ALL
        .into_iter()
        .map(|f| (f, f.extension(), f.label()))
        .collect()
}

/// The active timeline as text in `format`, with the report of what it
/// could not carry: what `interchange_export` writes.
fn render(state: &Arc<AppState>, format: Format) -> Result<(String, usize, Report), String> {
    let mut report = Report::default();
    let t = state.with_project(|p| timeline::collect(p, &mut report))?;
    let clips: usize = t.tracks.iter().map(|t| t.clips.len()).sum();
    if clips == 0 {
        return Err("the timeline has no clips from video, image or sound files to export".into());
    }
    let text = match format {
        Format::Otio => otio::write(&t, &mut report),
        Format::Edl => edl::write(&t, &mut report),
        Format::Fcpxml => fcpxml::write(&t, &mut report),
    };
    Ok((text, clips, report))
}

/// Write the active timeline for another editor. The format follows the
/// extension unless given; the extension is set to match the format.
pub fn interchange_export(
    state: &Arc<AppState>,
    path: &Path,
    format: Option<Format>,
) -> Result<InterchangeExport, String> {
    let format = format.or_else(|| Format::from_path(path)).ok_or_else(|| {
        format!(
            "{} is not an .otio, .fcpxml or .edl file; name the format",
            path.display()
        )
    })?;
    let path = path.with_extension(format.extension());
    let (text, clips, report) = render(state, format)?;
    // Written beside and renamed, so an editor watching the folder never
    // reads half a file.
    let partial = path.with_extension(format!("{}.partial", format.extension()));
    std::fs::write(&partial, text)
        .map_err(|e| format!("cannot write {}: {e}", partial.display()))?;
    std::fs::rename(&partial, &path).map_err(|e| {
        let _ = std::fs::remove_file(&partial);
        format!("cannot write {}: {e}", path.display())
    })?;
    Ok(InterchangeExport {
        path,
        format,
        clips,
        warnings: report.warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::interchange::timeline::tests::project;

    #[test]
    fn every_format_writes_a_file_with_its_own_extension() {
        let state = AppState::new();
        *state.project.write() = Some(project());
        let dir = std::env::temp_dir().join(format!("chukcut-interchange-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for format in Format::ALL {
            let out = interchange_export(&state, &dir.join("cut.txt"), Some(format)).unwrap();
            assert_eq!(out.path, dir.join(format!("cut.{}", format.extension())));
            assert_eq!(out.clips, 4);
            assert!(std::fs::metadata(&out.path).unwrap().len() > 100);
        }
        let err = interchange_export(&state, &dir.join("cut.txt"), None).unwrap_err();
        assert!(err.contains("name the format"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_empty_timeline_is_refused_in_words() {
        let state = AppState::new();
        let mut empty = project();
        empty.tracks.clear();
        *state.project.write() = Some(empty);
        let err = render(&state, Format::Otio).unwrap_err();
        assert!(err.contains("no clips"));
    }
}
