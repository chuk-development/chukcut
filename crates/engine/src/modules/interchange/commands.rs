//! The shell-facing API of timeline interchange.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;

use super::markers::{self, MarkerFormat};
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

/// What a marker export wrote.
#[derive(Debug, Clone, Serialize)]
pub struct MarkerExport {
    pub path: PathBuf,
    pub format: MarkerFormat,
    pub markers: usize,
    pub warnings: Vec<String>,
}

/// Write the active timeline's markers: as a spreadsheet (`.csv`), or as a
/// chapter list for a YouTube description (`.txt`). The format follows the
/// extension unless given.
pub fn interchange_export_markers(
    state: &Arc<AppState>,
    path: &Path,
    format: Option<MarkerFormat>,
) -> Result<MarkerExport, String> {
    let format = format
        .or_else(
            || match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
                "csv" => Some(MarkerFormat::Csv),
                "txt" => Some(MarkerFormat::Chapters),
                _ => None,
            },
        )
        .ok_or_else(|| {
            format!(
                "{} is not a .csv or .txt file; name the format",
                path.display()
            )
        })?;
    let path = path.with_extension(format.extension());
    let mut report = Report::default();
    let t = state.with_project(|p| timeline::collect(p, &mut Report::default()))?;
    if t.markers.is_empty() {
        return Err("the timeline has no markers; press M to add one at the playhead".into());
    }
    let text = match format {
        MarkerFormat::Csv => markers::csv(&t.name, &t.markers, t.rate),
        MarkerFormat::Chapters => markers::chapters(&t.markers, t.duration, &mut report),
    };
    std::fs::write(&path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(MarkerExport {
        path,
        format,
        markers: t.markers.len(),
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

    #[test]
    fn markers_go_out_as_a_spreadsheet_or_as_chapters() {
        let state = AppState::new();
        *state.project.write() = Some(project());
        let dir = std::env::temp_dir().join(format!("chukcut-markers-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let csv = interchange_export_markers(&state, &dir.join("notes.csv"), None).unwrap();
        assert_eq!(csv.markers, 1);
        let text = std::fs::read_to_string(&csv.path).unwrap();
        assert!(text.contains("\"drop\",\"00:00:06:00\""), "{text}");
        let chapters =
            interchange_export_markers(&state, &dir.join("yt"), Some(MarkerFormat::Chapters))
                .unwrap();
        assert_eq!(chapters.path, dir.join("yt.txt"));
        assert_eq!(
            std::fs::read_to_string(&chapters.path).unwrap(),
            "0:00 Intro\n0:06 drop\n"
        );
        // Two chapters, the last 2 s long: YouTube would refuse, and says so.
        assert_eq!(chapters.warnings.len(), 3, "{:?}", chapters.warnings);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
