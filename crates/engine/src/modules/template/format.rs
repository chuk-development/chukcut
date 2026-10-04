//! The template file: what a template *is* on disk.
//!
//! ```text
//! <id>/                     one directory per template
//!   template.json           the manifest, with the project inside it
//!   media/                  files the project uses that are not ours
//!                           (music, overlays, logos), copied in on save
//! ```
//!
//! `template.json` is `{ "format": "chukcut-template", "version": 1, id,
//! name, description, category, tags, cover_time, project }`, and `project`
//! is an ordinary `.chukcut` document — read through `project::migrate`, so
//! a template from an older build opens like an old project does. Its slots
//! are clips carrying a slot marker (`slot.rs`); their material is a
//! placeholder picture we draw (`assets.rs`). A media path that is relative
//! is relative to the template's directory. Decision 0022.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::modules::project::document::{Micros, Project};

/// The `format` tag. A file without it is not a template.
pub const FORMAT: &str = "chukcut-template";
/// The newest manifest this build writes and reads.
pub const VERSION: u32 = 1;
/// The manifest's name inside a template directory.
pub const MANIFEST: &str = "template.json";

/// The manifest and the project, as stored.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateFile {
    pub format: String,
    pub version: u32,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// The group the Templates tab files it under ("Social", "Travel").
    #[serde(default)]
    pub category: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// The instant the preview tile is drawn at; the middle of the first slot
    /// when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover_time: Option<Micros>,
    /// Kept as JSON so loading goes through `project::migrate`, which gates
    /// the schema version and repairs nulls, exactly as opening a project
    /// does.
    pub project: serde_json::Value,
}

impl TemplateFile {
    pub fn new(id: String, name: String, project: &Project) -> Result<Self, String> {
        Ok(Self {
            format: FORMAT.into(),
            version: VERSION,
            id,
            name,
            description: String::new(),
            category: String::new(),
            tags: Vec::new(),
            cover_time: None,
            project: serde_json::to_value(project).map_err(|e| e.to_string())?,
        })
    }

    /// The project inside, migrated, with relative media paths made absolute
    /// against `dir` when there is one.
    pub fn project(&self, dir: Option<&Path>) -> Result<Project, String> {
        let raw = self.project.to_string();
        let loaded = crate::modules::project::migrate::load(&raw)
            .map_err(|e| format!("the template's project cannot be read: {e}"))?;
        let mut project = loaded.project;
        if let Some(dir) = dir {
            resolve_paths(&mut project, dir);
        }
        Ok(project)
    }

    /// Parse and check a manifest.
    pub fn parse(raw: &str) -> Result<Self, String> {
        let file: TemplateFile =
            serde_json::from_str(raw).map_err(|e| format!("not a template: {e}"))?;
        if file.format != FORMAT {
            return Err(format!(
                "not a template: the format is {:?}, not {FORMAT:?}",
                file.format
            ));
        }
        if file.version > VERSION {
            return Err(format!(
                "this template was made by a newer chukcut (format version {}, this build reads {VERSION})",
                file.version
            ));
        }
        if file.id.trim().is_empty() {
            return Err("the template has no id".into());
        }
        Ok(file)
    }
}

/// Read the template in directory `dir`.
pub fn read_dir(dir: &Path) -> Result<TemplateFile, String> {
    let path = dir.join(MANIFEST);
    let raw = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    TemplateFile::parse(&raw).map_err(|e| format!("{}: {e}", path.display()))
}

/// Write `file` as `dir/template.json`, atomically.
pub fn write_dir(dir: &Path, file: &TemplateFile) -> Result<(), String> {
    let json = serde_json::to_string_pretty(file).map_err(|e| e.to_string())?;
    super::assets::write_atomically(&dir.join(MANIFEST), json.as_bytes())
}

/// Every media path in `project`, mutably: what bundling and resolving walk.
fn media_paths_mut(project: &mut Project) -> Vec<&mut String> {
    let pool = &mut project.materials;
    let mut paths: Vec<&mut String> = Vec::new();
    paths.extend(pool.videos.iter_mut().map(|m| &mut m.path));
    paths.extend(pool.audios.iter_mut().map(|m| &mut m.path));
    paths.extend(pool.images.iter_mut().map(|m| &mut m.path));
    paths.extend(
        pool.color_adjusts
            .iter_mut()
            .filter_map(|m| m.lut.as_mut().map(|lut| &mut lut.path)),
    );
    paths
}

/// Make every relative media path absolute against `dir`.
pub fn resolve_paths(project: &mut Project, dir: &Path) {
    for path in media_paths_mut(project) {
        if !path.is_empty() && Path::new(path.as_str()).is_relative() {
            *path = dir.join(path.as_str()).to_string_lossy().into_owned();
        }
    }
}

/// Copy every media file `project` uses into `dir/media/` and point the
/// project at the copies, relatively — except our own placeholders, music
/// beds and looks, which every installation can draw again (`assets.rs`,
/// `library::looks`). Returns how many files were copied.
///
/// A template must survive the folder its source footage lived in being
/// cleaned up, and be movable to another machine as one directory.
pub fn bundle_media(project: &mut Project, dir: &Path) -> Result<usize, String> {
    let ours = [
        super::assets::root(),
        crate::modules::workspace::paths::luts_dir(),
    ];
    let media = dir.join("media");
    let mut copied: Vec<(String, String)> = Vec::new();
    for path in media_paths_mut(project) {
        let source = PathBuf::from(path.as_str());
        if path.is_empty() || source.is_relative() || ours.iter().any(|o| source.starts_with(o)) {
            continue;
        }
        if let Some((_, relative)) = copied.iter().find(|(from, _)| from == path.as_str()) {
            *path = relative.clone();
            continue;
        }
        let name = source
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "media".into());
        // Two different files called `clip.mp4` must not overwrite each other.
        let mut target = media.join(&name);
        let mut n = 2;
        while copied.iter().any(|(_, rel)| dir.join(rel) == target) {
            target = media.join(format!("{n}-{name}"));
            n += 1;
        }
        std::fs::create_dir_all(&media)
            .map_err(|e| format!("cannot create {}: {e}", media.display()))?;
        std::fs::copy(&source, &target)
            .map_err(|e| format!("cannot copy {} into the template: {e}", source.display()))?;
        let relative = target
            .strip_prefix(dir)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| target.to_string_lossy().into_owned());
        copied.push((path.clone(), relative.clone()));
        *path = relative;
    }
    Ok(copied.len())
}

/// Copy every media file of `project` that lives under the template
/// directory `dir` into `to`, keeping its path below `dir`, and point the
/// project at the copies. Returns how many files were copied.
///
/// A project made from a user template must not read the template's
/// `media/` folder: deleting or moving the template would take those files
/// out of the project (they would go offline, decision 0009).
pub fn copy_out_media(project: &mut Project, dir: &Path, to: &Path) -> Result<usize, String> {
    let mut copied = 0;
    for path in media_paths_mut(project) {
        let source = PathBuf::from(path.as_str());
        let Ok(relative) = source.strip_prefix(dir) else {
            continue;
        };
        let target = to.join(relative);
        if !target.exists() {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
            }
            // Copied beside and renamed, so a half-copied file is never
            // mistaken for a finished one on the next try.
            let partial = target.with_extension(format!(
                "{}.part",
                target
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("media")
            ));
            std::fs::copy(&source, &partial)
                .map_err(|e| format!("cannot copy {} into the project: {e}", source.display()))?;
            std::fs::rename(&partial, &target)
                .map_err(|e| format!("cannot write {}: {e}", target.display()))?;
            copied += 1;
        }
        *path = target.to_string_lossy().into_owned();
    }
    Ok(copied)
}

/// A file-system-safe id from a name: lower case, dashes, and a short random
/// tail so two templates called "Intro" do not collide.
pub fn id_from_name(name: &str) -> String {
    let mut slug = String::new();
    for c in name.trim().chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-');
    let slug = if slug.is_empty() { "template" } else { slug };
    let tail: String = crate::modules::project::document::new_id()
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .take(6)
        .collect();
    format!("{}-{tail}", &slug[..slug.len().min(40)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::project::document::{CanvasConfig, ImageMaterial};

    #[test]
    fn a_manifest_from_a_newer_build_or_another_format_is_refused() {
        let p = Project::new("t", CanvasConfig::default(), 30.0);
        let mut file = TemplateFile::new("a".into(), "A".into(), &p).unwrap();
        let ok = serde_json::to_string(&file).unwrap();
        assert!(TemplateFile::parse(&ok).is_ok());

        file.version = VERSION + 1;
        let newer = serde_json::to_string(&file).unwrap();
        assert!(TemplateFile::parse(&newer)
            .unwrap_err()
            .contains("newer chukcut"));

        file.version = VERSION;
        file.format = "something".into();
        let other = serde_json::to_string(&file).unwrap();
        assert!(TemplateFile::parse(&other).unwrap_err().contains("format"));
    }

    #[test]
    fn relative_paths_resolve_against_the_template_directory() {
        let mut p = Project::new("t", CanvasConfig::default(), 30.0);
        p.materials.images.push(ImageMaterial {
            id: "i".into(),
            path: "media/logo.png".into(),
            width: 10,
            height: 10,
        });
        p.materials.images.push(ImageMaterial {
            id: "j".into(),
            path: "/abs/x.png".into(),
            width: 10,
            height: 10,
        });
        resolve_paths(&mut p, Path::new("/tpl/one"));
        assert_eq!(p.materials.images[0].path, "/tpl/one/media/logo.png");
        assert_eq!(p.materials.images[1].path, "/abs/x.png");
    }

    #[test]
    fn media_copied_out_of_a_template_no_longer_points_into_it() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/generated")
            .join(format!(
                "template-copy-out-{}",
                crate::modules::project::document::new_id()
            ));
        let template = root.join("template");
        let project_media = root.join("project");
        std::fs::create_dir_all(template.join("media")).unwrap();
        std::fs::write(template.join("media/logo.png"), b"logo").unwrap();

        let mut p = Project::new("t", CanvasConfig::default(), 30.0);
        for (id, path) in [
            ("i", template.join("media/logo.png")),
            ("j", PathBuf::from("/abs/elsewhere.png")),
        ] {
            p.materials.images.push(ImageMaterial {
                id: id.into(),
                path: path.to_string_lossy().into_owned(),
                width: 10,
                height: 10,
            });
        }
        assert_eq!(
            copy_out_media(&mut p, &template, &project_media).unwrap(),
            1
        );
        let copy = project_media.join("media/logo.png");
        assert_eq!(p.materials.images[0].path, copy.to_string_lossy());
        assert_eq!(p.materials.images[1].path, "/abs/elsewhere.png");

        // The template can go; the project keeps its picture.
        std::fs::remove_dir_all(&template).unwrap();
        assert_eq!(std::fs::read(&copy).unwrap(), b"logo");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn ids_are_slugs_with_a_tail() {
        let id = id_from_name("  My Travel  Intro! ");
        assert!(id.starts_with("my-travel-intro-"), "{id}");
        assert_eq!(id.len(), "my-travel-intro-".len() + 6);
        assert!(id_from_name("!!!").starts_with("template-"));
    }
}
