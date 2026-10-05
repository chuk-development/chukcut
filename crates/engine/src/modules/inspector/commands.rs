//! Commands for the inspector's crop and colour panels.
//!
//! Two commands, both thin: `edit.rs` builds the `EditCommand` and everything
//! goes through `History::apply`, so both edits sit on the ordinary undo
//! stack. The commands live here rather than the webview sending the composite
//! itself because the composite is built *from the document* — the segment's
//! current snapshot, its index on its track, which of its extras resolve as a
//! colour material — and a stale panel must be refused against the real
//! document, not against what it last saw.

use std::sync::Arc;

use super::edit::{self, ClipAttributes, ColorEdit, GradeControl, GradeEdit, GradeSection};
use crate::modules::project::document::{ColorAdjustMaterial, Crop, LutRef, Micros, Project};
use crate::modules::project::grade::{CurveChannel, Wheel, WheelKind};
use crate::modules::timeline::commands::EditResponse;
use crate::modules::timeline::ops::EditCommand;
use crate::state::AppState;

/// Set or clear a clip's crop rectangle. `None` clears it; so does the
/// full-frame rectangle, which keeps "uncropped" at one spelling.
pub fn inspector_set_crop(
    state: &Arc<AppState>,
    segment_id: String,
    crop: Option<Crop>,
) -> Result<EditResponse, String> {
    {
        let mut guard = state.project.write();
        let project = guard.as_mut().ok_or("no project is open")?;
        let command = edit::set_crop_command(project, &segment_id, crop)?;
        state.history.write().apply(project, command)?;
    }
    respond(state)
}

/// Make a clip show `crop` at the timeline instant `time`, as a keyframe on
/// each crop edge (added, or the one there changed). `None` keys the whole
/// picture. One undo step.
pub fn inspector_set_crop_at(
    state: &Arc<AppState>,
    segment_id: String,
    crop: Option<Crop>,
    time: Micros,
) -> Result<EditResponse, String> {
    {
        let mut guard = state.project.write();
        let project = guard.as_mut().ok_or("no project is open")?;
        let command = edit::crop_keyframe_command(project, &segment_id, crop, time)?;
        state.history.write().apply(project, command)?;
    }
    respond(state)
}

/// The crop's keyframe diamond at the timeline instant `time`: remove the
/// crop keyframes there, or add them holding the crop shown there.
pub fn inspector_toggle_crop_keyframe(
    state: &Arc<AppState>,
    segment_id: String,
    time: Micros,
) -> Result<EditResponse, String> {
    {
        let mut guard = state.project.write();
        let project = guard.as_mut().ok_or("no project is open")?;
        let command = edit::toggle_crop_keyframe_command(project, &segment_id, time)?;
        state.history.write().apply(project, command)?;
    }
    respond(state)
}

/// Set or clear a clip's colour adjustment. `None` — and the identity values —
/// clear it.
pub fn inspector_set_color(
    state: &Arc<AppState>,
    segment_id: String,
    color: Option<ColorEdit>,
) -> Result<EditResponse, String> {
    {
        let mut guard = state.project.write();
        let project = guard.as_mut().ok_or("no project is open")?;
        let (material, command) = edit::set_color_command(project, &segment_id, color)?;

        // The material goes into the pool before the command runs, the
        // ordering `text_add` documents: the document must never, even between
        // two writes, hold a segment referencing a material the pool lacks.
        // Direct rather than through a command for the reason given there — an
        // unreferenced material is inert — but unlike `text_add` this one is
        // taken back out if the edit is refused, because nothing else will
        // ever point at it.
        if let Some(material) = material {
            let id = material.id.clone();
            project.materials.color_adjusts.push(material);
            if let Err(error) = state.history.write().apply(project, command) {
                project.materials.color_adjusts.retain(|m| m.id != id);
                return Err(error);
            }
        } else {
            state.history.write().apply(project, command)?;
        }
    }
    respond(state)
}

/// Commit a grade edit built by one of the `edit::*_command` builders.
///
/// The material goes into the pool before the command runs, the ordering
/// `text_add` documents, and comes back out if the edit is refused — the
/// contract `inspector_set_color` spells out, shared by every grade command.
pub(crate) fn commit_grade(
    state: &Arc<AppState>,
    build: impl FnOnce(&Project) -> Result<(Option<ColorAdjustMaterial>, EditCommand), String>,
) -> Result<EditResponse, String> {
    {
        let mut guard = state.project.write();
        let project = guard.as_mut().ok_or("no project is open")?;
        let (material, command) = build(project)?;
        if let Some(material) = material {
            let id = material.id.clone();
            project.materials.color_adjusts.push(material);
            if let Err(error) = state.history.write().apply(project, command) {
                project.materials.color_adjusts.retain(|m| m.id != id);
                return Err(error);
            }
        } else {
            state.history.write().apply(project, command)?;
        }
    }
    respond(state)
}

/// Set a clip's whole grade — sliders, look, tone, HSL, curves, wheels — as
/// one undo step. `None`, and the identity, clear it.
pub fn inspector_set_grade(
    state: &Arc<AppState>,
    segment_id: String,
    grade: Option<GradeEdit>,
) -> Result<EditResponse, String> {
    commit_grade(state, |project| {
        edit::set_grade_command(project, &segment_id, grade)
    })
}

/// Set one control of a clip's grade, keeping the rest. The value is in
/// document units; see [`GradeControl`].
pub fn inspector_set_grade_control(
    state: &Arc<AppState>,
    segment_id: String,
    control: GradeControl,
    value: f32,
) -> Result<EditResponse, String> {
    commit_grade(state, |project| {
        edit::grade_control_command(project, &segment_id, control, value)
    })
}

/// Replace one tone curve's control points. An empty list resets it.
pub fn inspector_set_curve(
    state: &Arc<AppState>,
    segment_id: String,
    channel: CurveChannel,
    points: Vec<[f32; 2]>,
) -> Result<EditResponse, String> {
    commit_grade(state, |project| {
        edit::curve_command(project, &segment_id, channel, points)
    })
}

/// Set one colour wheel: the puck position and its luminance.
pub fn inspector_set_wheel(
    state: &Arc<AppState>,
    segment_id: String,
    kind: WheelKind,
    wheel: Wheel,
) -> Result<EditResponse, String> {
    commit_grade(state, |project| {
        edit::wheel_command(project, &segment_id, kind, wheel)
    })
}

/// Attach, swap or remove a clip's LUT, keeping the rest of the grade.
///
/// Like `inspector_set_color`, this does no IO: a path is checked by
/// [`inspector_lut_probe`] or [`inspector_lut_import`] when it is chosen.
pub fn inspector_set_lut(
    state: &Arc<AppState>,
    segment_id: String,
    lut: Option<LutRef>,
) -> Result<EditResponse, String> {
    commit_grade(state, |project| {
        edit::lut_command(project, &segment_id, lut)
    })
}

/// Put one section of a clip's grade back at rest.
pub fn inspector_reset_grade(
    state: &Arc<AppState>,
    segment_id: String,
    section: GradeSection,
) -> Result<EditResponse, String> {
    commit_grade(state, |project| {
        edit::reset_grade_command(project, &segment_id, section)
    })
}

/// Apply a copied clip's transform, speed, volume, crop and colour grade to
/// every clip in `segment_ids`, as one undo step.
///
/// The attribute values come from the webview's clipboard rather than from a
/// segment id, because the clipboard outlives the document it copied from; the
/// composite itself is built here against the real document, so a stale
/// selection is skipped and the grade lands as one shared immutable material.
pub fn inspector_paste_attributes(
    state: &Arc<AppState>,
    attributes: ClipAttributes,
    segment_ids: Vec<String>,
) -> Result<EditResponse, String> {
    {
        let mut guard = state.project.write();
        let project = guard.as_mut().ok_or("no project is open")?;
        let (material, command) =
            edit::paste_attributes_command(project, &attributes, &segment_ids)?;

        // Pool before command, with the take-back on failure: the same
        // contract as `inspector_set_color` above, for the same reasons.
        if let Some(material) = material {
            let id = material.id.clone();
            project.materials.color_adjusts.push(material);
            if let Err(error) = state.history.write().apply(project, command) {
                project.materials.color_adjusts.retain(|m| m.id != id);
                return Err(error);
            }
        } else {
            state.history.write().apply(project, command)?;
        }
    }
    respond(state)
}

/// Name a clip, or clear its name. The name lives in `MaterialPool::extras`
/// and is resolved by the webview's label code; see `edit::rename_clip_command`
/// for why a segment grows no field for it.
pub fn inspector_rename_clip(
    state: &Arc<AppState>,
    segment_id: String,
    name: Option<String>,
) -> Result<EditResponse, String> {
    {
        let mut guard = state.project.write();
        let project = guard.as_mut().ok_or("no project is open")?;
        let (entry, command) = edit::rename_clip_command(project, &segment_id, name)?;

        if let Some((id, value)) = entry {
            project.materials.extras.insert(id.clone(), value);
            if let Err(error) = state.history.write().apply(project, command) {
                project.materials.extras.remove(&id);
                return Err(error);
            }
        } else {
            state.history.write().apply(project, command)?;
        }
    }
    respond(state)
}

/// Play a clip — and every clip linked to it — at `speed`, keeping the part
/// of the file it shows: 2x halves its length on the timeline, and the clips
/// after it on the same lanes move up. One undo step. See
/// `edit::set_speed_command`.
pub fn inspector_set_speed(
    state: &Arc<AppState>,
    segment_id: String,
    speed: f32,
) -> Result<EditResponse, String> {
    {
        let mut guard = state.project.write();
        let project = guard.as_mut().ok_or("no project is open")?;
        let command = edit::set_speed_command(project, &segment_id, speed)?;
        state.history.write().apply(project, command)?;
    }
    respond(state)
}

/// Give every other picture clip the grade of `segment_id`, as one undo step.
/// See `edit::apply_color_to_all_command`.
pub fn inspector_apply_color_to_all(
    state: &Arc<AppState>,
    segment_id: String,
) -> Result<EditResponse, String> {
    {
        let mut guard = state.project.write();
        let project = guard.as_mut().ok_or("no project is open")?;
        let command = edit::apply_color_to_all_command(project, &segment_id)?;
        state.history.write().apply(project, command)?;
    }
    respond(state)
}

/// What the panel wants to know about a .cube file before attaching it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LutInfo {
    /// The file's `TITLE`, when it declares one.
    pub title: Option<String>,
    /// Entries per axis: edge length `N` of a cube, or the length of a 1D
    /// table.
    pub size: u32,
    /// Whether the file is a 1D table rather than a 3D cube.
    pub one_d: bool,
}

/// Read and parse a .cube file, without touching the document.
///
/// The one place a LUT file is validated *eagerly*: the picker calls this so
/// a malformed file is refused with the parser's line-numbered message at the
/// moment the user chooses it. `inspector_set_color` itself never does IO —
/// re-committing an intensity change must keep working after the file has
/// gone missing, because "missing LUT" is a warning state, not an error one.
pub fn inspector_lut_probe(path: String) -> Result<LutInfo, String> {
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("could not read {path}: {error}"))?;
    let cube = crate::modules::render::lut::parse(&text)
        .map_err(|error| format!("{} is not a usable LUT: {error}", file_name(&path)))?;
    Ok(LutInfo {
        title: cube.title,
        size: cube.size,
        one_d: cube.one_d,
    })
}

/// One file in the LUT library.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct LutEntry {
    /// The file name without `.cube`, which is what the picker shows.
    pub name: String,
    /// Absolute path, which is what a clip's `LutRef` stores.
    pub path: String,
}

/// The `.cube` files in the LUT library, sorted by name.
///
/// Lists without parsing: a library of a hundred 65-point cubes would take
/// seconds to validate, and every file in it was validated when it was
/// imported. A file someone dropped in by hand that turns out broken is
/// refused when it is attached, by [`inspector_lut_probe`].
pub fn inspector_lut_library() -> Vec<LutEntry> {
    lut_library_in(&crate::modules::workspace::paths::luts_dir())
}

fn lut_library_in(dir: &std::path::Path) -> Vec<LutEntry> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<LutEntry> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("cube"))
        })
        .map(|p| LutEntry {
            name: p
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
            path: p.to_string_lossy().into_owned(),
        })
        .collect();
    out.sort_by_key(|e| e.name.to_lowercase());
    out
}

/// Validate a `.cube` file and copy it into the LUT library.
///
/// A malformed file is refused with the parser's line-numbered message and
/// nothing is copied. A file whose name is taken by a *different* file gets
/// a numbered name; importing the same file twice returns the existing copy.
/// The library copy is what a clip references afterwards, so the look
/// survives the original being deleted from Downloads.
pub fn inspector_lut_import(path: String) -> Result<LutEntry, String> {
    lut_import_into(&path, &crate::modules::workspace::paths::luts_dir())
}

fn lut_import_into(path: &str, dir: &std::path::Path) -> Result<LutEntry, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("could not read {path}: {error}"))?;
    let text = String::from_utf8_lossy(&bytes);
    crate::modules::render::lut::parse(&text)
        .map_err(|error| format!("{} is not a usable LUT: {error}", file_name(path)))?;

    std::fs::create_dir_all(dir)
        .map_err(|error| format!("could not create the LUT library: {error}"))?;
    let source = std::path::Path::new(path);
    let stem = source
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "look".into());
    for n in 1.. {
        let name = if n == 1 {
            stem.clone()
        } else {
            format!("{stem} {n}")
        };
        let target = dir.join(format!("{name}.cube"));
        match std::fs::read(&target) {
            Ok(existing) if existing == bytes => {
                return Ok(LutEntry {
                    name,
                    path: target.to_string_lossy().into_owned(),
                })
            }
            Ok(_) => continue,
            Err(_) => {
                std::fs::write(&target, &bytes)
                    .map_err(|error| format!("could not copy the LUT: {error}"))?;
                return Ok(LutEntry {
                    name,
                    path: target.to_string_lossy().into_owned(),
                });
            }
        }
    }
    unreachable!("the loop only ends by returning")
}

fn file_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

/// The reply to an edit, with the working copy written on the way out.
///
/// A copy of `timeline::commands::respond`, which is private to that module —
/// the same duplication `text::commands` carries, for the same reason given
/// there.
fn respond(state: &AppState) -> Result<EditResponse, String> {
    let project = state.project.read().clone().ok_or("no project is open")?;
    let origin = state.project_path.read().clone();
    crate::modules::project::autosave::schedule(&project, origin);

    let history = state.history.read();
    Ok(EditResponse {
        project,
        can_undo: history.can_undo(),
        can_redo: history.can_redo(),
        undo_label: history.undo_label(),
        redo_label: history.redo_label(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("chukcut-lut-library-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn importing_copies_validates_and_names_without_clobbering() {
        let dir = scratch("import");
        let library = dir.join("library");
        let warm = dir.join("Warm Look.cube");
        std::fs::write(
            &warm,
            crate::modules::render::lut::fixtures::identity_cube(2),
        )
        .unwrap();

        let entry = lut_import_into(warm.to_str().unwrap(), &library).expect("imports");
        assert_eq!(entry.name, "Warm Look");
        assert!(std::path::Path::new(&entry.path).starts_with(&library));
        // The same file again is the same library entry, not a copy.
        let again = lut_import_into(warm.to_str().unwrap(), &library).unwrap();
        assert_eq!(again, entry);

        // A different file under the same name gets its own.
        let other = dir.join("other");
        std::fs::create_dir_all(&other).unwrap();
        let clash = other.join("Warm Look.cube");
        std::fs::write(
            &clash,
            crate::modules::render::lut::fixtures::table_text(4, |x| [x, x, x]),
        )
        .unwrap();
        let second = lut_import_into(clash.to_str().unwrap(), &library).unwrap();
        assert_eq!(second.name, "Warm Look 2");

        // The library outlives the original download.
        std::fs::remove_file(&warm).unwrap();
        let listed = lut_library_in(&library);
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].name, "Warm Look");
        assert!(inspector_lut_probe(listed[0].path.clone()).is_ok());
    }

    #[test]
    fn a_malformed_file_is_refused_with_a_readable_reason_and_not_copied() {
        let dir = scratch("malformed");
        let library = dir.join("library");
        let bad = dir.join("broken.cube");
        std::fs::write(&bad, "LUT_3D_SIZE 2\n0 0 0\n1 0 banana\n").unwrap();
        let error = lut_import_into(bad.to_str().unwrap(), &library).unwrap_err();
        assert!(
            error.contains("broken.cube") && error.contains("line 3"),
            "{error}"
        );
        assert!(lut_library_in(&library).is_empty());

        let error =
            lut_import_into(dir.join("absent.cube").to_str().unwrap(), &library).unwrap_err();
        assert!(error.contains("could not read"), "{error}");
    }
}
