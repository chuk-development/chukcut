//! The template commands: what the app, the CLI and the MCP server call.
//!
//! - [`template_list`], [`template_info`] — what is there.
//! - [`template_new_project`] — a new open project from a template, the
//!   user's files filling its slots in order.
//! - [`template_replace_media`] — put a file into one slot (or any picture
//!   clip) of the open project, as one undo step.
//! - [`template_slots`] — the open project's slots.
//! - [`template_save`] — the open project as a template of the user's own.
//! - [`template_delete`], [`template_thumbnail`].

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;

use crate::modules::project::commands::import_material;
use crate::modules::project::document::{MaterialKind, Micros, Project};
use crate::modules::timeline::commands::EditResponse;
use crate::state::AppState;

use super::fill::{self, FillKind, FillMedia, FillPlan};
use super::format::{self, TemplateFile};
use super::slot::{self, Slot, SlotMedia};
use super::{assets, builtin, save, thumb};

/// A template as a list shows it.
#[derive(Debug, Clone, Serialize)]
pub struct TemplateInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub category: String,
    /// Shipped with chukcut, rather than saved by the user.
    pub builtin: bool,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub duration: Micros,
    pub slots: Vec<SlotInfo>,
    /// The template's directory, for a user template.
    pub path: Option<String>,
}

/// One slot as a list shows it.
#[derive(Debug, Clone, Serialize)]
pub struct SlotInfo {
    pub index: u32,
    pub label: Option<String>,
    pub duration: Micros,
    pub aspect: [u32; 2],
    pub accepts: SlotMedia,
}

/// A template, loaded: its file, its project with paths resolved, and where
/// it came from.
pub struct Loaded {
    pub file: TemplateFile,
    pub project: Project,
    pub dir: Option<PathBuf>,
}

impl Loaded {
    fn info(&self) -> TemplateInfo {
        let project = &self.project;
        TemplateInfo {
            id: self.file.id.clone(),
            name: self.file.name.clone(),
            description: self.file.description.clone(),
            category: self.file.category.clone(),
            builtin: self.dir.is_none(),
            width: project.canvas.width,
            height: project.canvas.height,
            fps: project.fps,
            duration: project.duration(),
            slots: slot::slots(project)
                .into_iter()
                .map(|s| SlotInfo {
                    index: s.index,
                    label: s.label,
                    duration: s.duration,
                    aspect: s.aspect,
                    accepts: s.accepts,
                })
                .collect(),
            path: self.dir.as_ref().map(|d| d.to_string_lossy().into_owned()),
        }
    }

    /// The thumbnail cache key: what the tile depends on.
    fn tile_key(&self) -> String {
        match &self.dir {
            None => format!(
                "builtin:{}:{}:{}",
                self.file.id,
                builtin::BUILTIN_VERSION,
                env!("CARGO_PKG_VERSION")
            ),
            Some(_) => serde_json::to_string(&self.file).unwrap_or_default(),
        }
    }
}

/// The user templates under `root`, each a directory with a manifest. One
/// that does not read is skipped and logged.
fn user_templates(root: &Path) -> Vec<Loaded> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.join(format::MANIFEST).is_file())
        .collect();
    dirs.sort();
    dirs.into_iter()
        .filter_map(|dir| {
            let loaded = format::read_dir(&dir).and_then(|file| {
                let project = file.project(Some(&dir))?;
                Ok(Loaded {
                    file,
                    project,
                    dir: Some(dir.clone()),
                })
            });
            match loaded {
                Ok(loaded) => Some(loaded),
                Err(error) => {
                    tracing::warn!(dir = %dir.display(), %error, "a template does not read");
                    None
                }
            }
        })
        .collect()
}

/// Load template `id`: a built-in, or one of the user's.
pub fn load(id: &str) -> Result<Loaded, String> {
    load_from(id, &assets::user_dir())
}

fn load_from(id: &str, user_root: &Path) -> Result<Loaded, String> {
    if let Some(built) = builtin::get(id) {
        let file = built?;
        let project = file.project(None)?;
        return Ok(Loaded {
            file,
            project,
            dir: None,
        });
    }
    user_templates(user_root)
        .into_iter()
        .find(|t| t.file.id == id)
        .ok_or_else(|| {
            format!("there is no template called {id}; `chukcut-cli template list` lists them")
        })
}

/// Every template: the built-ins, then the user's.
pub fn template_list() -> Vec<TemplateInfo> {
    list_from(&assets::user_dir())
}

fn list_from(user_root: &Path) -> Vec<TemplateInfo> {
    let builtins = builtin::all().into_iter().filter_map(|file| {
        let project = file.project(None).ok()?;
        Some(
            Loaded {
                file,
                project,
                dir: None,
            }
            .info(),
        )
    });
    builtins
        .chain(user_templates(user_root).iter().map(Loaded::info))
        .collect()
}

pub fn template_info(id: &str) -> Result<TemplateInfo, String> {
    load(id).map(|t| t.info())
}

/// How one slot was filled.
#[derive(Debug, Clone, Serialize)]
pub struct FilledSlot {
    pub index: u32,
    pub segment_id: String,
    pub path: String,
    /// The clip was shorter than the slot and plays slowed down, at this
    /// speed.
    pub slowed_to: Option<f32>,
    pub cropped: bool,
}

/// What [`template_new_project`] made.
#[derive(Debug, Clone, Serialize)]
pub struct TemplateApplied {
    pub project: Project,
    pub filled: Vec<FilledSlot>,
    /// Slots still showing their placeholder, by index.
    pub empty: Vec<u32>,
}

fn kind_of(imported: &crate::modules::project::commands::ImportedMaterial) -> Option<FillKind> {
    match imported.kind {
        MaterialKind::Video => Some(FillKind::Video),
        MaterialKind::Image => Some(FillKind::Image),
        _ => None,
    }
}

/// Probe `path` and add it to `project`'s pool. Blocking.
fn import(project: &mut Project, path: &str) -> Result<FillMedia, String> {
    if !Path::new(path).is_file() {
        return Err(format!("{path} does not exist"));
    }
    let info = crate::modules::media::probe(path).map_err(|e| format!("{path}: {e}"))?;
    let name = Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string());
    let imported = import_material(project, path, &name, &info)?;
    let kind = kind_of(&imported)
        .ok_or_else(|| format!("{name} is sound only; a slot needs a video or a photo"))?;
    Ok(FillMedia {
        material_id: imported.id,
        kind,
        duration: imported.duration,
        width: imported.width,
        height: imported.height,
    })
}

/// Fill the slot clip `segment_id` of `project` with `media`, applying the
/// edit directly (no history). Shared by the new-project path, which builds a
/// document nobody has an undo stack for yet.
fn fill_in_place(
    project: &mut Project,
    segment_id: &str,
    media: &FillMedia,
) -> Result<FillPlan, String> {
    let ((marker_id, value), plan, command) =
        fill::replace_command(project, segment_id, media, None)?;
    project.materials.extras.insert(marker_id.clone(), value);
    if let Err(error) = command.apply(project) {
        project.materials.extras.remove(&marker_id);
        return Err(error);
    }
    Ok(plan)
}

/// Build the project template `id` makes with `media` in its slots, in
/// order, without opening it. Fewer files than slots leave the rest showing
/// their placeholder; more files than slots is refused, as is a file that
/// does not read or does not suit its slot — by name, so the user knows
/// which. Blocking: probes every file.
pub fn template_build_project(
    id: &str,
    media: &[String],
    name: Option<String>,
) -> Result<TemplateApplied, String> {
    build_from(load(id)?, media, name)
}

fn build_from(
    template: Loaded,
    media: &[String],
    name: Option<String>,
) -> Result<TemplateApplied, String> {
    let mut project = template.project;
    let slots = slot::slots(&project);
    if media.len() > slots.len() {
        return Err(format!(
            "{} has {} slot{}, and {} files were given",
            template.file.name,
            slots.len(),
            if slots.len() == 1 { "" } else { "s" },
            media.len()
        ));
    }
    project.id = crate::modules::project::document::new_id();
    project.name = name
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| template.file.name.clone());
    let now = chrono_now();
    project.created_at = now;
    project.updated_at = now;
    project.canvas_chosen = true;
    // A user template's own media (a logo, a sound) is copied out of its
    // directory, so deleting or moving the template leaves the project whole.
    if let Some(dir) = &template.dir {
        let to = assets::project_media_dir(&project.id);
        format::copy_out_media(&mut project, dir, &to)?;
    }

    let mut filled = Vec::new();
    for (slot, path) in slots.iter().zip(media) {
        let fill_media = import(&mut project, path)?;
        let plan = fill_in_place(&mut project, &slot.segment_id, &fill_media)
            .map_err(|e| format!("slot {} ({path}): {e}", slot.index))?;
        filled.push(FilledSlot {
            index: slot.index,
            segment_id: slot.segment_id.clone(),
            path: path.clone(),
            slowed_to: plan.slowed.then_some(plan.speed),
            cropped: plan.crop.is_some(),
        });
    }
    let empty = slots.iter().skip(media.len()).map(|s| s.index).collect();
    // Placeholders for the empty slots, the music, the looks: on disk before
    // anything renders the project.
    assets::ensure_for(&project)?;
    crate::modules::library::commands::library_looks_install()?;
    Ok(TemplateApplied {
        project,
        filled,
        empty,
    })
}

fn chrono_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// [`template_build_project`], then make it the open project — what "Use
/// template" does. Like `project_new`, the history starts empty and the
/// working copy is written at once. Blocking.
pub fn template_new_project(
    state: &Arc<AppState>,
    id: &str,
    media: &[String],
    name: Option<String>,
) -> Result<TemplateApplied, String> {
    let applied = template_build_project(id, media, name)?;
    template_open_project(state, &applied.project);
    Ok(applied)
}

/// Make a project [`template_build_project`] built the open one: the second
/// half of [`template_new_project`], for a shell that builds off its UI
/// thread and opens on it.
pub fn template_open_project(state: &Arc<AppState>, project: &Project) {
    *state.project.write() = Some(project.clone());
    *state.project_path.write() = None;
    state.history.write().clear();
    crate::modules::project::autosave::schedule(project, None);
    let pool = &project.materials;
    let paths: Vec<String> = pool
        .videos
        .iter()
        .map(|m| m.path.clone())
        .chain(pool.audios.iter().map(|m| m.path.clone()))
        .chain(pool.images.iter().map(|m| m.path.clone()))
        .collect();
    crate::modules::workspace::commands::workspace_cache_in_use(paths);
    crate::modules::proxy::commands::proxy_request_media(
        pool.videos.iter().map(|m| m.path.clone()).collect(),
    );
}

/// The slots of the open project, in fill order.
pub fn template_slots(state: &Arc<AppState>) -> Result<Vec<Slot>, String> {
    state.with_project(slot::slots)
}

/// What [`template_replace_media`] did.
#[derive(Serialize)]
pub struct Replaced {
    pub edit: EditResponse,
    pub slowed_to: Option<f32>,
    pub cropped: bool,
}

/// Put the file at `path` into the clip `segment_id` — a slot, or any video
/// or photo clip — keeping the clip's place, length and look. One undo
/// step. `source_start` picks where a longer clip starts. Blocking: probes
/// the file, without the project lock held.
pub fn template_replace_media(
    state: &Arc<AppState>,
    segment_id: &str,
    path: &str,
    source_start: Option<Micros>,
) -> Result<Replaced, String> {
    if !Path::new(path).is_file() {
        return Err(format!("{path} does not exist"));
    }
    let info = crate::modules::media::probe(path).map_err(|e| format!("{path}: {e}"))?;
    let name = Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string());
    let plan = {
        let mut guard = state.project.write();
        let project = guard.as_mut().ok_or("no project is open")?;
        // Importing is not undoable anywhere (`project_import_media`); an
        // unused material is inert.
        let imported = import_material(project, path, &name, &info)?;
        let kind = kind_of(&imported)
            .ok_or_else(|| format!("{name} is sound only; a slot needs a video or a photo"))?;
        let media = FillMedia {
            material_id: imported.id,
            kind,
            duration: imported.duration,
            width: imported.width,
            height: imported.height,
        };
        let ((marker_id, value), plan, command) =
            fill::replace_command(project, segment_id, &media, source_start)?;
        project.materials.extras.insert(marker_id.clone(), value);
        if let Err(error) = state.history.write().apply(project, command) {
            project.materials.extras.remove(&marker_id);
            return Err(error);
        }
        plan
    };
    if info.has_video {
        crate::modules::proxy::commands::proxy_request_media(vec![path.to_string()]);
    }
    Ok(Replaced {
        edit: crate::modules::voice::commands::respond(state)?,
        slowed_to: plan.slowed.then_some(plan.speed),
        cropped: plan.crop.is_some(),
    })
}

/// What to save: see [`save::SaveRequest`].
pub use save::SaveRequest;

/// Save the open project as a template of the user's own, the clips in
/// `request.slots` becoming its slots. Copies the media it keeps into the
/// template's directory. Blocking.
pub fn template_save(state: &Arc<AppState>, request: &SaveRequest) -> Result<TemplateInfo, String> {
    let project = state.project.read().clone().ok_or("no project is open")?;
    save_into(&project, request, &assets::user_dir())
}

/// [`template_save`] for a project that is not open — the CLI's path.
pub fn template_save_project(
    project: &Project,
    request: &SaveRequest,
) -> Result<TemplateInfo, String> {
    save_into(project, request, &assets::user_dir())
}

fn save_into(
    project: &Project,
    request: &SaveRequest,
    user_root: &Path,
) -> Result<TemplateInfo, String> {
    let mut file = save::make_template(project, request)?;
    let dir = user_root.join(&file.id);
    let mut stored = file.project(None)?;
    assets::ensure_for(&stored)?;
    format::bundle_media(&mut stored, &dir)?;
    file.project = serde_json::to_value(&stored).map_err(|e| e.to_string())?;
    format::write_dir(&dir, &file)?;
    let project = file.project(Some(&dir))?;
    Ok(Loaded {
        file,
        project,
        dir: Some(dir),
    }
    .info())
}

/// Delete one of the user's templates. A built-in cannot be deleted.
pub fn template_delete(id: &str) -> Result<(), String> {
    let loaded = load(id)?;
    let dir = loaded.dir.ok_or_else(|| {
        format!(
            "{} ships with chukcut and cannot be deleted",
            loaded.file.name
        )
    })?;
    // Only ever a directory under the user templates root, whatever the
    // manifest claims.
    if dir.parent() != Some(assets::user_dir().as_path()) {
        return Err("that template is not in the templates folder".into());
    }
    std::fs::remove_dir_all(&dir).map_err(|e| format!("cannot delete {}: {e}", dir.display()))
}

/// The preview tile of template `id`, short edge `short` pixels, rendered
/// once and cached. Blocking and GPU-bound: call it off the UI thread.
pub fn template_thumbnail(id: &str, short: u32) -> Result<PathBuf, String> {
    let loaded = load(id)?;
    thumb::render(&loaded.file, &loaded.project, &loaded.tile_key(), short)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_has_the_builtins_and_reads_user_templates_from_disk() {
        // Under the crate's ignored fixture folder, not the system temp dir.
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/generated")
            .join(format!(
                "template-list-{}",
                crate::modules::project::document::new_id()
            ));
        let before = list_from(&root);
        assert!(before.iter().all(|t| t.builtin));
        assert!(before.len() >= 8);

        // A template saved from a built-in lands in the list, with its slots.
        let quick = load_from("quick-cuts", &root).unwrap();
        let request = SaveRequest {
            name: "My cuts".into(),
            ..SaveRequest::default()
        };
        let mut file = save::make_template(&quick.project, &request).unwrap();
        let dir = root.join(&file.id);
        let mut stored = file.project(None).unwrap();
        format::bundle_media(&mut stored, &dir).unwrap();
        file.project = serde_json::to_value(&stored).unwrap();
        format::write_dir(&dir, &file).unwrap();

        let after = list_from(&root);
        let mine = after.iter().find(|t| t.name == "My cuts").unwrap();
        assert!(!mine.builtin);
        assert_eq!(mine.slots.len(), 6);
        assert_eq!(mine.category, "My templates");
        let again = load_from(&mine.id, &root).unwrap();
        assert_eq!(slot::slots(&again.project).len(), 6);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn more_files_than_slots_is_refused_before_anything_is_read() {
        let quick = load_from("talking-points", Path::new("/nonexistent")).unwrap();
        let error = build_from(quick, &["/a.mp4".into(), "/b.mp4".into()], None).unwrap_err();
        assert!(error.contains("1 slot,"), "{error}");
    }

    #[test]
    fn a_split_slot_stays_one_slot_on_the_left_half() {
        let mut project = load_from("quick-cuts", Path::new("/nonexistent"))
            .unwrap()
            .project;
        let before = slot::slots(&project);
        let first = before[0].clone();
        let cut = first.start + first.duration / 2;
        let command =
            crate::modules::timeline::ops::split_at(&project, &first.segment_id, cut).unwrap();
        command.apply(&mut project).unwrap();

        let after = slot::slots(&project);
        assert_eq!(after.len(), before.len());
        assert_eq!(after[0].segment_id, first.segment_id);
        assert_eq!(after[0].duration, cut - first.start);
        // The right half is a plain clip that names no marker.
        let (track, _) = project.segment(&first.segment_id).unwrap();
        let right = track
            .segments
            .iter()
            .find(|s| s.target_range.start == cut)
            .unwrap();
        assert!(slot::marker_of(&project, right).is_none());
    }

    #[test]
    fn a_missing_file_is_named() {
        let quick = load_from("quick-cuts", Path::new("/nonexistent")).unwrap();
        let error = build_from(quick, &["/no/such/clip.mp4".into()], None).unwrap_err();
        assert!(error.contains("/no/such/clip.mp4"), "{error}");
    }
}
