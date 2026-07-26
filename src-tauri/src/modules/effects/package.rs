//! The package loader: open a package, parse its manifests, expose its
//! parameters.
//!
//! A package is a directory or a zip. Both are behind [`PackageSource`] so that
//! nothing above this file cares which — a downloaded `.zip` is read in place
//! rather than being unpacked into a temporary directory, because a package can
//! carry several hundred files and only a handful are ever read.
//!
//! ## What is authoritative for what
//!
//! `docs/research/effect-package-format.md` establishes the layering, and it is
//! worth restating because it is not the obvious one:
//!
//! - `config.json` is the only file always present. Its `effect.Link[]` names
//!   the feature directories and their order. It **may be absent or empty** —
//!   §2.6's Lynx-Studio family has no `effect` block at all, and §2.7's
//!   model-only packages declare `"Link": []`. Neither is an error.
//! - **`lua-meta.json` / `js-meta.json` are the parameter manifests.** The
//!   2026-05 spec dismissed them as usually empty; the 2026-07 survey of 209
//!   packages found 109 and 91 of them respectively, and they are the compiled
//!   form of the `---@field ... [UI(...)]` annotations in the source. Parse
//!   these; do not parse Lua comments.
//! - `extra.json` and `Link[].extra.composer_param[]` declare *host* sliders —
//!   a different layer, aimed at the editor's own UI rather than at a script
//!   property. Both are surfaced here, tagged by [`ParameterOrigin`], because
//!   the inspector has to show them together and the runtime has to route them
//!   differently: a script property is written to the instance, a host knob is
//!   broadcast as an event.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum PackageError {
    #[error("{0}")]
    Io(String),
    #[error("{path}: {message}")]
    Json { path: String, message: String },
    #[error("this is not an effect package: no config.json at its root")]
    NotAPackage,
    #[error("{0}")]
    Unsupported(String),
}

pub type Result<T> = std::result::Result<T, PackageError>;

/// A directory or a zip, read the same way.
enum PackageSource {
    Directory(PathBuf),
    Zip(parking_lot::Mutex<zip::ZipArchive<std::fs::File>>),
}

impl PackageSource {
    fn read(&self, relative: &str) -> Result<Vec<u8>> {
        match self {
            Self::Directory(root) => {
                let path = safe_join(root, relative)?;
                std::fs::read(&path)
                    .map_err(|e| PackageError::Io(format!("{}: {e}", path.display())))
            }
            Self::Zip(archive) => {
                // The guard is bound rather than used inline: `by_name` borrows
                // the archive, so a temporary guard would be dropped while the
                // entry is still alive.
                let mut guard = archive.lock();
                // Zip members are stored with forward slashes and packages are
                // inconsistent about a leading `./`.
                let name = relative.trim_start_matches("./");
                let mut buffer = Vec::new();
                let mut file = guard
                    .by_name(name)
                    .map_err(|e| PackageError::Io(format!("{name} in the zip: {e}")))?;
                file.read_to_end(&mut buffer)
                    .map_err(|e| PackageError::Io(format!("{name} in the zip: {e}")))?;
                Ok(buffer)
            }
        }
    }

    fn exists(&self, relative: &str) -> bool {
        match self {
            Self::Directory(root) => safe_join(root, relative)
                .map(|p| p.exists())
                .unwrap_or(false),
            Self::Zip(archive) => {
                let mut guard = archive.lock();
                // Bound rather than returned inline: the `ZipFile` borrows the
                // guard and would otherwise still be alive when the guard drops
                // at the end of the block.
                let found = guard.by_name(relative.trim_start_matches("./")).is_ok();
                found
            }
        }
    }
}

/// A package is untrusted input from a URL the user pasted, so a member path
/// that climbs out of the package root is refused rather than followed. This is
/// the same class of hole as zip-slip and it costs one function to close.
fn safe_join(root: &Path, relative: &str) -> Result<PathBuf> {
    let mut path = root.to_path_buf();
    for part in relative.split('/') {
        match part {
            "" | "." => continue,
            ".." => {
                return Err(PackageError::Unsupported(format!(
                    "the package refers to {relative}, which points outside it"
                )))
            }
            other => path.push(other),
        }
    }
    Ok(path)
}

// ---------------------------------------------------------------------------
// config.json
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Config {
    #[serde(default)]
    pub effect: Option<Effect>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    /// `"AmazingEditor"` or `"AEExporter:<x.y.z>"`.
    #[serde(default)]
    pub ae_tool: Option<String>,
    /// Lynx-Studio family (§2.6). `0` means the `.lsanim` beside it is plain
    /// JSON rather than encrypted.
    #[serde(default)]
    pub encrypt: Option<i64>,
    #[serde(default)]
    pub script_type: Option<String>,
    #[serde(default)]
    pub studio_animation_path: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Effect {
    #[serde(default, rename = "Link")]
    pub link: Vec<Link>,
    #[serde(default)]
    pub model_names: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub force_render: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Link {
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub path: String,
    /// Integer in most packages and a float (`8011.0`) in some, so it is read
    /// as one and compared as one.
    #[serde(default)]
    pub zorder: f64,
    #[serde(default)]
    pub default_enable: Option<bool>,
    #[serde(default, rename = "preRenderAlgorithmTex")]
    pub pre_render_algorithm_tex: Option<bool>,
    #[serde(default, rename = "rtShare")]
    pub rt_share: Option<bool>,
    #[serde(default, rename = "needBlend")]
    pub need_blend: Option<bool>,
    #[serde(default)]
    pub extra: Option<LinkExtra>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct LinkExtra {
    #[serde(default)]
    pub composer_param: Vec<ComposerParam>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ComposerParam {
    pub name: String,
    pub key: String,
    #[serde(default)]
    pub default_value: f64,
    #[serde(default)]
    pub min_value: Option<f64>,
    #[serde(default)]
    pub max_value: Option<f64>,
    #[serde(default)]
    pub allow_negative: Option<bool>,
}

// ---------------------------------------------------------------------------
// <Link>/content.json
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Content {
    #[serde(default)]
    pub tag: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub needblend: Option<bool>,
    /// §2.2. When `filemap.prefab` is present that prefab is the scene root and
    /// there may be no `main.scene` at all.
    #[serde(default)]
    pub filemap: BTreeMap<String, String>,
}

// ---------------------------------------------------------------------------
// extra.json
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AdjustParam {
    pub effect_key: String,
    #[serde(default)]
    pub default: f64,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
}

// ---------------------------------------------------------------------------
// lua-meta.json / js-meta.json
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ScriptClass {
    #[serde(rename = "ClassName")]
    pub class_name: String,
    #[serde(default, rename = "Super")]
    pub super_name: String,
    #[serde(default, rename = "FilePath")]
    pub file_path: String,
    #[serde(default, rename = "Properties")]
    pub properties: Vec<ScriptProperty>,
    #[serde(default, rename = "Comment")]
    pub comment: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ScriptProperty {
    #[serde(rename = "VarName")]
    pub var_name: String,
    #[serde(default, rename = "VarType")]
    pub var_type: String,
    #[serde(default, rename = "Comment")]
    pub comment: String,
    #[serde(default, rename = "AnnoItems")]
    pub anno_items: Vec<AnnoItem>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AnnoItem {
    #[serde(default, rename = "ItemType")]
    pub item_type: String,
    #[serde(default, rename = "Attributes")]
    pub attributes: Vec<AnnoAttribute>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AnnoAttribute {
    #[serde(default, rename = "AttrType")]
    pub attr_type: String,
    #[serde(default, rename = "RawValue")]
    pub raw_value: String,
    #[serde(default, rename = "Values")]
    pub values: Vec<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// The parameter model the UI sees
// ---------------------------------------------------------------------------

/// Where a parameter came from, because the runtime routes them differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParameterOrigin {
    /// A `---@field` on a script class, compiled into `lua-meta.json`. Written
    /// straight onto the script instance's property table.
    ScriptProperty,
    /// `extra.json`'s `effect_adjust_params[]`. Broadcast to the script's
    /// `onEvent` as `(effect_key, value)`.
    HostAdjust,
    /// `config.json`'s `Link[].extra.composer_param[]`. Same broadcast, but the
    /// range is explicit rather than normalised.
    ComposerParam,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParameterKind {
    Float,
    Int,
    Bool,
    String,
    Vec2,
    Vec3,
    Vec4,
    Color,
    Texture,
    Material,
    Mesh,
    Transform,
    /// A declared type we do not model. Kept rather than dropped so the
    /// inspector can grey it out and the user can see the effect is only
    /// partly supported.
    Unknown,
}

impl ParameterKind {
    /// The `VarType` vocabulary from `lua-meta.json`, §8.2. The names differ
    /// from the source-annotation vocabulary in §8.1 (`double` vs `Double`),
    /// so both spellings are accepted.
    fn from_var_type(name: &str) -> Self {
        match name.to_ascii_lowercase().as_str() {
            "double" | "float" | "number" => Self::Float,
            "int" | "int64" | "integer" => Self::Int,
            "bool" | "boolean" => Self::Bool,
            "string" => Self::String,
            "vector2f" | "vector2" => Self::Vec2,
            "vector3f" | "vector3" | "vector" => Self::Vec3,
            "vector4f" | "vector4" => Self::Vec4,
            "color" => Self::Color,
            "texture" => Self::Texture,
            "material" => Self::Material,
            "mesh" => Self::Mesh,
            "transform" => Self::Transform,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Widget {
    Slider,
    Drag,
    /// `[UI(Option={"Add", "Multiply", …})]` — the choices are in
    /// [`Parameter::options`].
    Choice,
    /// `[UI(Button)]` renders an action, not a value.
    Button,
    Default,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ParameterRange {
    pub min: f64,
    pub max: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Parameter {
    /// The name to write. A script property name, or an `effect_key`.
    pub key: String,
    /// `[UI(Display="…")]` when it exists, otherwise the key.
    pub label: String,
    pub kind: ParameterKind,
    pub widget: Widget,
    pub origin: ParameterOrigin,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<ParameterRange>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<f64>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub options: Vec<String>,
    /// `[UI(Order=N)]`. Stable sort key for the inspector.
    pub order: i64,
    /// Which script class declared it, for a `ScriptProperty`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
}

// ---------------------------------------------------------------------------
// The package
// ---------------------------------------------------------------------------

pub struct EffectPackage {
    source: PackageSource,
    origin: String,
    config: Config,
    /// Per link, keyed by the link's index in `config.effect.Link`.
    contents: Vec<Option<Content>>,
    parameters: Vec<Parameter>,
}

impl std::fmt::Debug for EffectPackage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EffectPackage")
            .field("origin", &self.origin)
            .field("links", &self.config.effect.as_ref().map(|e| e.link.len()))
            .field("parameters", &self.parameters.len())
            .finish()
    }
}

impl EffectPackage {
    /// Open a package from a directory or a `.zip`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let source = if path.is_dir() {
            PackageSource::Directory(path.to_path_buf())
        } else {
            let file = std::fs::File::open(path)
                .map_err(|e| PackageError::Io(format!("{}: {e}", path.display())))?;
            let archive = zip::ZipArchive::new(file)
                .map_err(|e| PackageError::Io(format!("{}: not a zip: {e}", path.display())))?;
            PackageSource::Zip(parking_lot::Mutex::new(archive))
        };

        if !source.exists("config.json") {
            return Err(PackageError::NotAPackage);
        }

        let config: Config = read_json(&source, "config.json")?;

        let links = config.effect.as_ref().map(|e| e.link.clone()).unwrap_or_default();
        let mut contents = Vec::with_capacity(links.len());
        for link in &links {
            let path = join_link(&link.path, "content.json");
            contents.push(if source.exists(&path) {
                Some(read_json(&source, &path)?)
            } else {
                None
            });
        }

        let mut package = Self {
            source,
            origin: path.display().to_string(),
            config,
            contents,
            parameters: Vec::new(),
        };
        package.parameters = package.collect_parameters()?;
        Ok(package)
    }

    pub fn origin(&self) -> &str {
        &self.origin
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn name(&self) -> &str {
        self.config.name.as_deref().unwrap_or("effect")
    }

    /// The links in the order they are applied — by `zorder`, which is not
    /// necessarily the order they appear in the file.
    pub fn links(&self) -> Vec<&Link> {
        let mut links: Vec<&Link> = self
            .config
            .effect
            .as_ref()
            .map(|e| e.link.iter().collect())
            .unwrap_or_default();
        links.sort_by(|a, b| a.zorder.partial_cmp(&b.zorder).unwrap_or(std::cmp::Ordering::Equal));
        links
    }

    pub fn content(&self, link_index: usize) -> Option<&Content> {
        self.contents.get(link_index).and_then(|c| c.as_ref())
    }

    /// The scene root for a link: the prefab named by `content.json`'s
    /// `filemap`, or `main.scene`. §2.2 — both forms coexist and some packages
    /// carry both.
    pub fn scene_root(&self, link_index: usize) -> Option<String> {
        let link = self.config.effect.as_ref()?.link.get(link_index)?;
        if let Some(content) = self.content(link_index) {
            if let Some(prefab) = content.filemap.get("prefab") {
                return Some(join_link(&link.path, prefab.trim_start_matches('/')));
            }
        }
        let scene = join_link(&link.path, "main.scene");
        self.source.exists(&scene).then_some(scene)
    }

    /// Read a member of the package by its package-relative path.
    pub fn read(&self, relative: &str) -> Result<Vec<u8>> {
        self.source.read(relative)
    }

    pub fn read_to_string(&self, relative: &str) -> Result<String> {
        let bytes = self.read(relative)?;
        String::from_utf8(bytes)
            .map_err(|e| PackageError::Io(format!("{relative} is not UTF-8: {e}")))
    }

    pub fn exists(&self, relative: &str) -> bool {
        self.source.exists(relative)
    }

    /// Every declared parameter, from all three mechanisms, sorted for display.
    pub fn parameters(&self) -> &[Parameter] {
        &self.parameters
    }

    fn collect_parameters(&self) -> Result<Vec<Parameter>> {
        let mut out = Vec::new();

        // 1. extra.json — host sliders for the whole effect (§8.3).
        if self.source.exists("extra.json") {
            let value: serde_json::Value = read_json(&self.source, "extra.json")?;
            if let Some(setting) = value.get("setting") {
                // Three shapes coexist under `setting`, and one package uses a
                // bare scalar at top level. Every array of objects that carries
                // an `effect_key` is a slider list, whatever the array is named
                // — `effect_adjust_params` and `bloom_adjust_params` are both
                // observed and there is no reason to think that list is closed.
                if let Some(map) = setting.as_object() {
                    for (_, entry) in map {
                        let Some(array) = entry.as_array() else { continue };
                        for item in array {
                            let Ok(param) =
                                serde_json::from_value::<AdjustParam>(item.clone())
                            else {
                                continue;
                            };
                            out.push(Parameter {
                                label: param.effect_key.clone(),
                                key: param.effect_key,
                                kind: ParameterKind::Float,
                                widget: Widget::Slider,
                                origin: ParameterOrigin::HostAdjust,
                                range: Some(ParameterRange {
                                    min: param.min.unwrap_or(0.0),
                                    max: param.max.unwrap_or(1.0),
                                }),
                                default: Some(param.default),
                                options: Vec::new(),
                                order: 0,
                                class: None,
                            });
                        }
                    }
                }
            }
        }

        // 2. config.json → Link[].extra.composer_param[] (§8.3).
        for link in self.config.effect.iter().flat_map(|e| &e.link) {
            for param in link.extra.iter().flat_map(|e| &e.composer_param) {
                out.push(Parameter {
                    label: param.name.clone(),
                    key: param.key.clone(),
                    kind: ParameterKind::Float,
                    widget: Widget::Slider,
                    origin: ParameterOrigin::ComposerParam,
                    range: Some(ParameterRange {
                        min: param.min_value.unwrap_or(0.0),
                        max: param.max_value.unwrap_or(1.0),
                    }),
                    default: Some(param.default_value),
                    options: Vec::new(),
                    order: 0,
                    class: None,
                });
            }
        }

        // 3. lua-meta.json / js-meta.json — the authoritative script-property
        //    manifests (§8.2).
        for link in self.config.effect.iter().flat_map(|e| &e.link) {
            for manifest in ["lua-meta.json", "js-meta.json"] {
                let path = join_link(&link.path, manifest);
                if !self.source.exists(&path) {
                    continue;
                }
                // A build artifact rather than a curated file, so a malformed
                // one is not a reason to refuse the whole package: the effect
                // still renders, it just has no editable parameters.
                let classes: Vec<ScriptClass> = match read_json(&self.source, &path) {
                    Ok(classes) => classes,
                    Err(error) => {
                        tracing::warn!(%path, %error, "ignoring an unreadable parameter manifest");
                        continue;
                    }
                };
                for class in classes {
                    for property in class.properties {
                        out.push(parameter_from_property(&class.class_name, property));
                    }
                }
            }
        }

        out.sort_by(|a, b| a.order.cmp(&b.order).then_with(|| a.label.cmp(&b.label)));
        Ok(out)
    }
}

fn parameter_from_property(class: &str, property: ScriptProperty) -> Parameter {
    let mut widget = Widget::Default;
    let mut range = None;
    let mut options = Vec::new();
    let mut order = 0i64;
    let mut label = property.var_name.clone();

    for item in &property.anno_items {
        if !item.item_type.eq_ignore_ascii_case("UI") {
            continue;
        }
        for attribute in &item.attributes {
            match attribute.attr_type.as_str() {
                "Range" => {
                    let values: Vec<f64> =
                        attribute.values.iter().filter_map(|v| v.as_f64()).collect();
                    if values.len() >= 2 {
                        range = Some(ParameterRange {
                            min: values[0],
                            max: values[1],
                        });
                    }
                }
                "Slider" => widget = Widget::Slider,
                // `Drag` and `Slider` can both be present; a slider is the more
                // specific of the two and wins.
                "Drag" => {
                    if widget == Widget::Default {
                        widget = Widget::Drag;
                    }
                }
                "Button" => widget = Widget::Button,
                "Option" => {
                    widget = Widget::Choice;
                    options = attribute
                        .values
                        .iter()
                        .map(|v| match v.as_str() {
                            Some(s) => s.to_string(),
                            None => v.to_string(),
                        })
                        .collect();
                }
                "Display" => {
                    if let Some(text) = attribute.values.first().and_then(|v| v.as_str()) {
                        label = text.to_string();
                    } else if !attribute.raw_value.is_empty() {
                        label = attribute.raw_value.clone();
                    }
                }
                "Order" => {
                    if let Some(n) = attribute.values.first().and_then(|v| v.as_f64()) {
                        order = n as i64;
                    }
                }
                _ => {}
            }
        }
    }

    Parameter {
        key: property.var_name,
        label,
        kind: ParameterKind::from_var_type(&property.var_type),
        widget,
        origin: ParameterOrigin::ScriptProperty,
        range,
        default: None,
        options,
        order,
        class: Some(class.to_string()),
    }
}

/// Link paths carry a trailing slash in most packages, none in some, and are
/// `""` in a few (§3.1: "May be `\"\"`").
pub(crate) fn join_link(link_path: &str, relative: &str) -> String {
    let base = link_path.trim_end_matches('/');
    if base.is_empty() {
        relative.to_string()
    } else {
        format!("{base}/{relative}")
    }
}

fn read_json<T: serde::de::DeserializeOwned>(source: &PackageSource, path: &str) -> Result<T> {
    let bytes = source.read(path)?;
    serde_json::from_slice(&bytes).map_err(|e| PackageError::Json {
        path: path.to_string(),
        message: e.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/modules/effects/fixtures")
            .join(name)
    }

    #[test]
    fn opens_a_directory_package_and_orders_links_by_zorder() {
        let package = EffectPackage::open(fixture("tint")).unwrap();
        assert_eq!(package.name(), "chukcut_tint");
        let links = package.links();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].kind, "AmazingFeature");
    }

    #[test]
    fn reads_parameters_from_lua_meta_rather_than_from_lua_comments() {
        let package = EffectPackage::open(fixture("tint")).unwrap();
        let intensity = package
            .parameters()
            .iter()
            .find(|p| p.key == "intensity")
            .expect("the lua-meta.json declares intensity");

        assert_eq!(intensity.kind, ParameterKind::Float);
        assert_eq!(intensity.widget, Widget::Slider);
        assert_eq!(intensity.origin, ParameterOrigin::ScriptProperty);
        assert_eq!(
            intensity.range,
            Some(ParameterRange { min: 0.0, max: 1.0 })
        );
        // `[UI(Display="Strength")]` is the human label; the key stays the
        // variable name because that is what gets written to the instance.
        assert_eq!(intensity.label, "Strength");
    }

    #[test]
    fn surfaces_host_sliders_and_script_properties_together_but_tagged_apart() {
        let package = EffectPackage::open(fixture("tint")).unwrap();
        let host = package
            .parameters()
            .iter()
            .find(|p| p.origin == ParameterOrigin::HostAdjust)
            .expect("extra.json declares one");
        assert_eq!(host.key, "effects_adjust_intensity");
        assert_eq!(host.default, Some(0.9));
        assert!(package
            .parameters()
            .iter()
            .any(|p| p.origin == ParameterOrigin::ScriptProperty));
    }

    #[test]
    fn a_package_with_no_effect_block_is_not_an_error() {
        // §2.6's Lynx-Studio family and §2.7's model-only packages both have
        // no usable `Link[]`, and refusing them would refuse a real format.
        let package = EffectPackage::open(fixture("empty_links")).unwrap();
        assert!(package.links().is_empty());
        assert!(package.parameters().is_empty());
    }

    #[test]
    fn a_member_path_that_climbs_out_of_the_package_is_refused() {
        let package = EffectPackage::open(fixture("tint")).unwrap();
        let error = package.read("../../../etc/passwd").unwrap_err();
        assert!(matches!(error, PackageError::Unsupported(_)));
    }

    #[test]
    fn a_zip_and_a_directory_of_the_same_package_read_alike() {
        let directory = EffectPackage::open(fixture("tint")).unwrap();
        let zipped = zip_up(&fixture("tint"));
        let archive = EffectPackage::open(&zipped).unwrap();

        assert_eq!(directory.name(), archive.name());
        assert_eq!(
            directory.parameters().len(),
            archive.parameters().len(),
            "the same package read two ways declares the same parameters"
        );
        assert_eq!(
            directory.read_to_string("AmazingFeature/xshader/tint.frag").unwrap(),
            archive.read_to_string("AmazingFeature/xshader/tint.frag").unwrap()
        );
    }

    /// Zip the fixture into a temporary file. Written here rather than
    /// committed so the repository holds one copy of the fixture, in the form
    /// that is reviewable.
    fn zip_up(root: &Path) -> PathBuf {
        use std::io::Write;
        let out = std::env::temp_dir().join(format!("chukcut-effect-{}.zip", std::process::id()));
        let file = std::fs::File::create(&out).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options: zip::write::FileOptions<()> = zip::write::FileOptions::default();
        for entry in walk(root) {
            let name = entry
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            writer.start_file(name, options).unwrap();
            writer.write_all(&std::fs::read(&entry).unwrap()).unwrap();
        }
        writer.finish().unwrap();
        out
    }

    fn walk(root: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    out.push(path);
                }
            }
        }
        out
    }
}
