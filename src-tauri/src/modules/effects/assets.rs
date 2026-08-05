//! `%SerializedFormat%@` assets: `.xshader`, `.material`, `.rt`, `.mesh`,
//! `.scene`, `.prefab`.
//!
//! ## The one thing to know before reading this file
//!
//! These assets ship in **two interchangeable encodings** of the same object
//! model (`effect-package-format.md` §4.1): a proprietary binary starting with
//! the literal `%SerializedFormat%@`, and **YAML 1.1 with typed document
//! tags**. The engine loads both, and a minority of every asset type in the
//! surveyed cache is plain text — 24 of 210 `.rt` files, 4 of 263 `.material`,
//! 4 of 213 `.xshader`.
//!
//! Only the YAML form is read here. That is a deliberate stopping point, not
//! an oversight: the binary container is a self-describing object graph with a
//! type registry, and decoding it is a piece of work in its own right worth
//! several days. Reading the YAML first is what makes that work *cheap* — it
//! documents, exactly and for free, which fields the binary must be made to
//! yield. A binary asset is refused by [`SerializedFile::parse`] with a message
//! saying so, rather than being half-parsed into something that renders wrong.
//!
//! ## The object model
//!
//! One file is a list of documents. Each carries a type tag and an anchor:
//!
//! ```yaml
//! --- !XShader &1
//! passes:
//!   - shaders: {gles2: [{localId: 4}, {localId: 5}]}
//! --- !Shader &4
//! type: {__class: ShaderType, value: VERTEX}
//! sourcePath: xshader/quad.vert
//! ```
//!
//! A reference is `{localId: N}` — document `N` in *this* file — or
//! `{localId: N, path: "relative/path"}` — document `N` in *that* file. Every
//! object also carries a `guid: {a, b}`, which is the identity used across
//! files; nothing here needs it yet, and it is preserved rather than dropped so
//! a future scene loader can.
//!
//! Enum values are `{__class: SomeEnum, value: NAME}` and maps are
//! `{__class: Map, ...}`, both of which are read through [`enum_value`] and by
//! ignoring the `__class` key.

use serde_yaml_ng::Value;
use std::collections::BTreeMap;

#[derive(Debug, thiserror::Error)]
pub enum AssetError {
    #[error(
        "{0} is in the binary %SerializedFormat%@ encoding, which this runtime does not read yet. \
         The same asset in its YAML form loads; see modules/effects/assets.rs."
    )]
    BinaryContainer(String),
    #[error("{path}: {message}")]
    Malformed { path: String, message: String },
    #[error("{path}: no {kind} document in the file")]
    Missing { path: String, kind: &'static str },
}

pub type Result<T> = std::result::Result<T, AssetError>;

/// The magic that says "this is the binary encoding". §4.1.
pub const BINARY_MAGIC: &[u8] = b"%SerializedFormat%@";

/// One parsed file: its documents, keyed by anchor.
#[derive(Debug, Clone)]
pub struct SerializedFile {
    pub path: String,
    /// `(tag, anchor, body)` in file order.
    pub documents: Vec<(String, u64, Value)>,
}

impl SerializedFile {
    pub fn parse(path: &str, bytes: &[u8]) -> Result<Self> {
        if bytes.starts_with(BINARY_MAGIC) {
            return Err(AssetError::BinaryContainer(path.to_string()));
        }
        let text = std::str::from_utf8(bytes).map_err(|e| AssetError::Malformed {
            path: path.to_string(),
            message: format!("not UTF-8: {e}"),
        })?;

        let mut documents = Vec::new();
        for (tag, anchor, body) in split_documents(text) {
            // Inline type tags (`!<str> 40` inside `enabledMacros`) carry no
            // information a typed reader needs and libyaml would demand a
            // resolver for them.
            let body = body.replace("!<str> ", "");
            let value: Value =
                serde_yaml_ng::from_str(&body).map_err(|e| AssetError::Malformed {
                    path: path.to_string(),
                    message: format!("document !{tag}: {e}"),
                })?;
            documents.push((tag, anchor, value));
        }
        Ok(Self {
            path: path.to_string(),
            documents,
        })
    }

    pub fn first(&self, tag: &str) -> Option<&Value> {
        self.documents
            .iter()
            .find(|(t, _, _)| t == tag)
            .map(|(_, _, v)| v)
    }

    pub fn by_anchor(&self, anchor: u64) -> Option<(&str, &Value)> {
        self.documents
            .iter()
            .find(|(_, a, _)| *a == anchor)
            .map(|(t, _, v)| (t.as_str(), v))
    }

    fn require(&self, tag: &'static str) -> Result<&Value> {
        self.first(tag).ok_or(AssetError::Missing {
            path: self.path.clone(),
            kind: tag,
        })
    }
}

/// Split on document headers, keeping each header's `!Tag` and `&anchor`.
///
/// Done by hand rather than through the YAML parser because serde's YAML
/// layer discards document tags, and the tag is the *type* of the object —
/// the single most important thing in the file.
fn split_documents(text: &str) -> Vec<(String, u64, String)> {
    let mut out: Vec<(String, u64, String)> = Vec::new();
    let mut current: Option<(String, u64, String)> = None;

    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("---") {
            if let Some(doc) = current.take() {
                out.push(doc);
            }
            let mut tag = String::new();
            let mut anchor = 0u64;
            for word in rest.split_whitespace() {
                if let Some(t) = word.strip_prefix('!') {
                    tag = t.to_string();
                } else if let Some(a) = word.strip_prefix('&') {
                    anchor = a.parse().unwrap_or(0);
                }
            }
            current = Some((tag, anchor, String::new()));
            continue;
        }
        if line.starts_with('%') {
            continue; // %YAML 1.1
        }
        if let Some((_, _, body)) = current.as_mut() {
            body.push_str(line);
            body.push('\n');
        }
    }
    if let Some(doc) = current.take() {
        out.push(doc);
    }
    out
}

// ---------------------------------------------------------------------------
// Value helpers
// ---------------------------------------------------------------------------

fn get<'v>(value: &'v Value, key: &str) -> Option<&'v Value> {
    value.get(key)
}

fn as_f32(value: Option<&Value>) -> Option<f32> {
    value?.as_f64().map(|v| v as f32)
}

fn as_u32(value: Option<&Value>) -> Option<u32> {
    value?.as_u64().map(|v| v as u32)
}

fn as_str(value: Option<&Value>) -> Option<&str> {
    value?.as_str()
}

/// `{__class: SomeEnum, value: NAME}` → `"NAME"`.
fn enum_value(value: Option<&Value>) -> Option<&str> {
    as_str(get(value?, "value"))
}

/// A reference: `{localId: N}` or `{localId: N, path: "…"}`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AssetRef {
    pub local_id: u64,
    /// `None` means "in this file".
    pub path: Option<String>,
}

impl AssetRef {
    fn read(value: Option<&Value>) -> Option<Self> {
        let value = value?;
        Some(Self {
            local_id: get(value, "localId")?.as_u64()?,
            path: as_str(get(value, "path")).map(str::to_string),
        })
    }
}

fn vector(value: Option<&Value>, keys: &[&str]) -> Option<Vec<f32>> {
    let value = value?;
    keys.iter().map(|k| as_f32(get(value, k))).collect()
}

/// The map objects are `{__class: Map, key: value, …}`; `__class` is the only
/// key that is not data.
fn map_entries(value: Option<&Value>) -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    let Some(Value::Mapping(mapping)) = value else {
        return out;
    };
    for (key, item) in mapping {
        let Some(key) = key.as_str() else { continue };
        if key == "__class" {
            continue;
        }
        out.insert(key.to_string(), item.clone());
    }
    out
}

// ---------------------------------------------------------------------------
// .xshader
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShaderType {
    Vertex,
    Fragment,
}

#[derive(Debug, Clone)]
pub struct ShaderSource {
    pub kind: ShaderType,
    /// Package-relative, e.g. `xshader/GaussianBlur.vert`.
    pub source_path: String,
    /// `!Shader` objects can carry a `macros` list, which is how one `.frag`
    /// becomes several compiled variants (§4.4).
    pub macros: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct BlendState {
    pub enabled: bool,
    pub src_color: String,
    pub dst_color: String,
    pub src_alpha: String,
    pub dst_alpha: String,
}

#[derive(Debug, Clone)]
pub struct Pass {
    pub name: String,
    pub vertex: Option<ShaderSource>,
    pub fragment: Option<ShaderSource>,
    /// Attribute name → vertex semantic (`POSITION`, `USER_DEFINE1`, …). A
    /// loader binds by semantic rather than guessing from the name.
    pub semantics: BTreeMap<String, String>,
    /// This pass's own target. `None` means "the camera's".
    pub render_texture: Option<AssetRef>,
    pub clear_color: [f32; 4],
    pub clear: bool,
    pub blend: BlendState,
    pub depth_test: bool,
    pub depth_write: bool,
}

#[derive(Debug, Clone)]
pub struct XShader {
    pub name: String,
    pub render_queue: i64,
    pub passes: Vec<Pass>,
}

impl XShader {
    pub fn parse(path: &str, bytes: &[u8]) -> Result<Self> {
        let file = SerializedFile::parse(path, bytes)?;
        let root = file.require("XShader")?;

        let name = as_str(get(root, "name")).unwrap_or(path).to_string();
        let render_queue = get(root, "renderQueue")
            .and_then(Value::as_i64)
            .unwrap_or(3000);

        let mut passes = Vec::new();
        let empty = Vec::new();
        for entry in get(root, "passes")
            .and_then(Value::as_sequence)
            .unwrap_or(&empty)
        {
            passes.push(parse_pass(&file, entry));
        }

        Ok(Self {
            name,
            render_queue,
            passes,
        })
    }
}

fn parse_pass(file: &SerializedFile, entry: &Value) -> Pass {
    let name = as_str(get(entry, "name")).unwrap_or("pass").to_string();

    // `shaders` is keyed by backend tag. `gles2` is the only one any shipped
    // package actually provides source for — `shaderHLSL5` and `shaderVulkan`
    // directories exist in the ZIPs and are empty in every case (§5.3) — so
    // the GLSL ES entry is preferred and the others are a fallback that has
    // never been exercised.
    let shaders = get(entry, "shaders");
    let refs: Vec<AssetRef> = ["gles2", "glsl20", "glsl30", "glsl31", "glsl32"]
        .iter()
        .find_map(|backend| {
            let list = get(shaders?, backend)?.as_sequence()?;
            Some(
                list.iter()
                    .filter_map(|v| AssetRef::read(Some(v)))
                    .collect::<Vec<_>>(),
            )
        })
        .unwrap_or_default();

    let mut vertex = None;
    let mut fragment = None;
    for reference in refs {
        if reference.path.is_some() {
            continue; // a shader in another file; not seen in the corpus
        }
        let Some((_, document)) = file.by_anchor(reference.local_id) else {
            continue;
        };
        let kind = match enum_value(get(document, "type")) {
            Some("VERTEX") => ShaderType::Vertex,
            Some("FRAGMENT") => ShaderType::Fragment,
            _ => continue,
        };
        let source = ShaderSource {
            kind,
            source_path: as_str(get(document, "sourcePath"))
                .unwrap_or_default()
                .to_string(),
            macros: get(document, "macros")
                .and_then(Value::as_sequence)
                .map(|s| {
                    s.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
        };
        match kind {
            ShaderType::Vertex => vertex = Some(source),
            ShaderType::Fragment => fragment = Some(source),
        }
    }

    let semantics = map_entries(get(entry, "semantics"))
        .into_iter()
        .filter_map(|(k, v)| enum_value(Some(&v)).map(|s| (k, s.to_string())))
        .collect();

    let clear_color = vector(get(entry, "clearColor"), &["r", "g", "b", "a"])
        .map(|v| [v[0], v[1], v[2], v[3]])
        .unwrap_or([0.0; 4]);

    let blend = get(entry, "renderState")
        .and_then(|state| get(state, "colorBlend"))
        .and_then(|cb| get(cb, "attachments"))
        .and_then(Value::as_sequence)
        .and_then(|a| a.first())
        .map(|a| BlendState {
            enabled: get(a, "blendEnable")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            src_color: enum_value(get(a, "srcColorBlendFactor"))
                .unwrap_or("ONE")
                .to_string(),
            dst_color: enum_value(get(a, "dstColorBlendFactor"))
                .unwrap_or("ZERO")
                .to_string(),
            src_alpha: enum_value(get(a, "srcAlphaBlendFactor"))
                .unwrap_or("ONE")
                .to_string(),
            dst_alpha: enum_value(get(a, "dstAlphaBlendFactor"))
                .unwrap_or("ZERO")
                .to_string(),
        })
        .unwrap_or_default();

    let depth = get(entry, "renderState").and_then(|s| get(s, "depthstencil"));

    Pass {
        name,
        vertex,
        fragment,
        semantics,
        render_texture: AssetRef::read(get(entry, "renderTexture")),
        clear_color,
        clear: matches!(
            enum_value(get(entry, "clearType")),
            Some("COLOR") | Some("ALL")
        ),
        blend,
        depth_test: depth
            .and_then(|d| get(d, "depthTestEnable"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        depth_write: depth
            .and_then(|d| get(d, "depthWriteEnable"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }
}

// ---------------------------------------------------------------------------
// .material
// ---------------------------------------------------------------------------

/// A texture binding resolves to one of three things (§4.3): another pass's
/// framebuffer, a file in the package, or a share URI naming a host input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextureSource {
    /// `share://input.texture` — the source video frame. This is the seam
    /// between the effect and the compositor.
    HostInput,
    /// `share://skinsegmask.texture` and friends.
    HostShare(String),
    /// A `.rt`, `.texture` or `.png` inside the package.
    Package(String),
}

impl TextureSource {
    fn parse(path: &str) -> Self {
        match path {
            "share://input.texture" => Self::HostInput,
            other if other.starts_with("share://") => {
                Self::HostShare(other.trim_start_matches("share://").to_string())
            }
            other => Self::Package(other.to_string()),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Material {
    pub name: String,
    pub xshader: Option<AssetRef>,
    pub floats: BTreeMap<String, f32>,
    pub ints: BTreeMap<String, i64>,
    pub vec2: BTreeMap<String, [f32; 2]>,
    pub vec3: BTreeMap<String, [f32; 3]>,
    pub vec4: BTreeMap<String, [f32; 4]>,
    pub textures: BTreeMap<String, TextureSource>,
    pub render_queue: i64,
    /// Preprocessor macros the shader is compiled with. `AE_*LightNum` is in
    /// every material and matters to nothing we render.
    pub macros: BTreeMap<String, String>,
}

impl Material {
    pub fn parse(path: &str, bytes: &[u8]) -> Result<Self> {
        let file = SerializedFile::parse(path, bytes)?;
        let root = file.require("Material")?;
        let properties = get(root, "properties");

        let mut material = Self {
            name: as_str(get(root, "name")).unwrap_or(path).to_string(),
            xshader: AssetRef::read(get(root, "xshader")),
            render_queue: get(root, "renderQueue")
                .and_then(Value::as_i64)
                .unwrap_or(3000),
            ..Default::default()
        };

        for (key, value) in map_entries(properties.and_then(|p| get(p, "floatmap"))) {
            if let Some(v) = value.as_f64() {
                material.floats.insert(key, v as f32);
            }
        }
        for (key, value) in map_entries(properties.and_then(|p| get(p, "intmap"))) {
            if let Some(v) = value.as_i64() {
                material.ints.insert(key, v);
            }
        }
        for (key, value) in map_entries(properties.and_then(|p| get(p, "vec2map"))) {
            if let Some(v) = vector(Some(&value), &["x", "y"]) {
                material.vec2.insert(key, [v[0], v[1]]);
            }
        }
        for (key, value) in map_entries(properties.and_then(|p| get(p, "vec3map"))) {
            if let Some(v) = vector(Some(&value), &["x", "y", "z"]) {
                material.vec3.insert(key, [v[0], v[1], v[2]]);
            }
        }
        for (key, value) in map_entries(properties.and_then(|p| get(p, "vec4map"))) {
            if let Some(v) = vector(Some(&value), &["x", "y", "z", "w"]) {
                material.vec4.insert(key, [v[0], v[1], v[2], v[3]]);
            }
        }
        for (key, value) in map_entries(properties.and_then(|p| get(p, "texmap"))) {
            if let Some(path) = as_str(get(&value, "path")) {
                material.textures.insert(key, TextureSource::parse(path));
            }
        }
        for (key, value) in map_entries(get(root, "enabledMacros")) {
            let text = value
                .as_str()
                .map(str::to_string)
                .or_else(|| value.as_i64().map(|v| v.to_string()))
                .unwrap_or_default();
            material.macros.insert(key, text);
        }

        Ok(material)
    }
}

// ---------------------------------------------------------------------------
// .rt
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct RenderTargetDesc {
    pub name: String,
    pub width: u32,
    pub height: u32,
    /// Resolution scale relative to the input frame. §4.5 — `pecentX`, spelt
    /// that way in the format.
    pub percent: [f32; 2],
    /// `true` marks a target that may be aliased with another pass's.
    pub shared: bool,
    pub filter_linear: bool,
    pub clamp: bool,
}

impl RenderTargetDesc {
    pub fn parse(path: &str, bytes: &[u8]) -> Result<Self> {
        let file = SerializedFile::parse(path, bytes)?;
        let root = file
            .first("ScreenRenderTexture")
            .or_else(|| file.first("RenderTexture"))
            .ok_or(AssetError::Missing {
                path: path.to_string(),
                kind: "ScreenRenderTexture",
            })?;
        Ok(Self {
            name: as_str(get(root, "name")).unwrap_or(path).to_string(),
            width: as_u32(get(root, "width")).unwrap_or(0),
            height: as_u32(get(root, "height")).unwrap_or(0),
            percent: [
                as_f32(get(root, "pecentX")).unwrap_or(1.0),
                as_f32(get(root, "pecentY")).unwrap_or(1.0),
            ],
            shared: get(root, "shared")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            filter_linear: !matches!(enum_value(get(root, "filterMin")), Some("NEAREST")),
            clamp: !matches!(enum_value(get(root, "wrapModeS")), Some("REPEAT")),
        })
    }

    /// The pixel size this target wants for a given input frame. `width`/
    /// `height` are authored against one resolution; `pecentX`/`pecentY` are
    /// what actually scale with the frame, so they win when they are not 1.
    pub fn size_for(&self, frame: (u32, u32)) -> (u32, u32) {
        let scaled = (
            (frame.0 as f32 * self.percent[0]).round() as u32,
            (frame.1 as f32 * self.percent[1]).round() as u32,
        );
        (scaled.0.max(1), scaled.1.max(1))
    }
}

// ---------------------------------------------------------------------------
// .mesh
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct MeshAsset {
    pub name: String,
    /// Interleaved, in the order `vertexAttribs` declares.
    pub vertices: Vec<f32>,
    pub attributes: Vec<String>,
    pub indices: Vec<u16>,
}

impl MeshAsset {
    pub fn parse(path: &str, bytes: &[u8]) -> Result<Self> {
        let file = SerializedFile::parse(path, bytes)?;
        let root = file.require("Mesh")?;
        let vertices = get(root, "vertices")
            .and_then(Value::as_sequence)
            .map(|s| {
                s.iter()
                    .filter_map(|v| v.as_f64())
                    .map(|v| v as f32)
                    .collect()
            })
            .unwrap_or_default();
        let attributes = get(root, "vertexAttribs")
            .and_then(Value::as_sequence)
            .map(|s| {
                s.iter()
                    .filter_map(|a| enum_value(get(a, "semantic")).map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let indices = get(root, "submeshes")
            .and_then(Value::as_sequence)
            .and_then(|s| s.first())
            .and_then(|s| get(s, "indices16"))
            .and_then(Value::as_sequence)
            .map(|s| {
                s.iter()
                    .filter_map(|v| v.as_u64())
                    .map(|v| v as u16)
                    .collect()
            })
            .unwrap_or_default();
        Ok(Self {
            name: as_str(get(root, "name")).unwrap_or(path).to_string(),
            vertices,
            attributes,
            indices,
        })
    }
}

// ---------------------------------------------------------------------------
// .scene / .prefab
// ---------------------------------------------------------------------------

/// What a scene contributes that this runtime uses: which materials get drawn
/// and which scripts drive them.
///
/// This is deliberately *not* a scene graph. The full ECS — transforms,
/// cameras with `renderOrder` and `layerVisibleMask`, prefab instancing — is
/// the piece of work after this one. What a single-material effect needs from
/// the scene is the material path, the script path and its scene-side property
/// defaults, and those are read here so a package's own entry point is
/// honoured rather than guessed at.
#[derive(Debug, Clone, Default)]
pub struct SceneEntry {
    pub materials: Vec<String>,
    pub meshes: Vec<String>,
    pub scripts: Vec<ScriptComponentDesc>,
}

#[derive(Debug, Clone)]
pub struct ScriptComponentDesc {
    /// Package-relative `.lua` path.
    pub path: String,
    /// The global table name the file exports.
    pub class_name: String,
    /// The values *this instance* starts with, overriding the defaults in
    /// `.new()`. §8.4.
    pub properties: BTreeMap<String, Value>,
}

impl SceneEntry {
    pub fn parse(path: &str, bytes: &[u8]) -> Result<Self> {
        let file = SerializedFile::parse(path, bytes)?;
        let root =
            file.first("Scene")
                .or_else(|| file.first("Prefab"))
                .ok_or(AssetError::Missing {
                    path: path.to_string(),
                    kind: "Scene or Prefab",
                })?;

        let mut entry = Self::default();
        let empty = Vec::new();
        let entities = get(root, "entities")
            .and_then(Value::as_sequence)
            .unwrap_or(&empty);
        for entity in entities {
            let components = get(entity, "components")
                .and_then(Value::as_sequence)
                .unwrap_or(&empty);
            for component in components {
                match as_str(get(component, "__class")) {
                    Some("MeshRenderer") => {
                        if let Some(materials) =
                            get(component, "sharedMaterials").and_then(Value::as_sequence)
                        {
                            for reference in materials {
                                if let Some(path) = as_str(get(reference, "path")) {
                                    entry.materials.push(path.to_string());
                                }
                            }
                        }
                        if let Some(path) =
                            as_str(get(component, "mesh").and_then(|m| get(m, "path")))
                        {
                            entry.meshes.push(path.to_string());
                        }
                    }
                    Some("ScriptComponent") => {
                        let Some(path) = as_str(get(component, "path")) else {
                            continue;
                        };
                        entry.scripts.push(ScriptComponentDesc {
                            path: path.to_string(),
                            class_name: as_str(get(component, "className"))
                                .unwrap_or("Script")
                                .to_string(),
                            properties: map_entries(get(component, "properties")),
                        });
                    }
                    _ => {}
                }
            }
        }
        Ok(entry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_binary_asset_is_refused_by_name_rather_than_half_parsed() {
        let mut bytes = BINARY_MAGIC.to_vec();
        bytes.extend_from_slice(&[0x0a, 0x02, 0, 0, 0]);
        let error = XShader::parse("xshader/pass0.xshader", &bytes).unwrap_err();
        assert!(matches!(error, AssetError::BinaryContainer(_)));
        assert!(error.to_string().contains("pass0.xshader"));
    }

    const XSHADER: &str = r#"%YAML 1.1
--- !XShader &1
name: tint/xshader
renderQueue: 3090
passes:
  - __class: Pass
    name: Tint
    shaders:
      __class: Map
      gles2: [{localId: 4}, {localId: 5}]
    semantics:
      __class: Map
      attPosition: {__class: VertexAttribType, value: POSITION}
      attUV: {__class: VertexAttribType, value: TEXCOORD0}
    renderTexture: {localId: 1, path: rt/midRT.rt}
    clearColor: {r: 0, g: 0, b: 0, a: 0}
    clearType: {__class: CameraClearType, value: COLOR}
    renderState:
      __class: RenderState
      depthstencil:
        depthTestEnable: false
        depthWriteEnable: false
      colorBlend:
        attachments:
          - blendEnable: true
            srcColorBlendFactor: {__class: BlendFactor, value: ONE}
            dstColorBlendFactor: {__class: BlendFactor, value: ONE_MINUS_SRC_ALPHA}
            srcAlphaBlendFactor: {__class: BlendFactor, value: ONE}
            dstAlphaBlendFactor: {__class: BlendFactor, value: ONE_MINUS_SRC_ALPHA}
--- !Shader &4
type: {__class: ShaderType, value: VERTEX}
sourcePath: xshader/tint.vert
--- !Shader &5
type: {__class: ShaderType, value: FRAGMENT}
sourcePath: xshader/tint.frag
macros: [SAMPLETIIMES1]
"#;

    #[test]
    fn an_xshader_resolves_its_shader_documents_by_local_id() {
        // The pass names `{localId: 4}` and `{localId: 5}`; the shader type
        // and the source path live in *those* documents, not in the pass.
        let shader = XShader::parse("xshader/tint.xshader", XSHADER.as_bytes()).unwrap();
        assert_eq!(shader.render_queue, 3090);
        assert_eq!(shader.passes.len(), 1);

        let pass = &shader.passes[0];
        assert_eq!(pass.name, "Tint");
        assert_eq!(
            pass.vertex.as_ref().unwrap().source_path,
            "xshader/tint.vert"
        );
        assert_eq!(
            pass.fragment.as_ref().unwrap().source_path,
            "xshader/tint.frag"
        );
        assert_eq!(pass.fragment.as_ref().unwrap().macros, ["SAMPLETIIMES1"]);
    }

    #[test]
    fn a_pass_carries_its_own_target_state_and_semantics() {
        // §4.4: "the shader files alone never fully describe a pass".
        let shader = XShader::parse("xshader/tint.xshader", XSHADER.as_bytes()).unwrap();
        let pass = &shader.passes[0];
        assert_eq!(
            pass.render_texture.as_ref().unwrap().path.as_deref(),
            Some("rt/midRT.rt")
        );
        assert!(pass.clear);
        assert!(pass.blend.enabled);
        assert_eq!(pass.blend.dst_color, "ONE_MINUS_SRC_ALPHA");
        assert!(!pass.depth_test);
        assert_eq!(pass.semantics.get("attPosition").unwrap(), "POSITION");
        assert_eq!(pass.semantics.get("attUV").unwrap(), "TEXCOORD0");
    }

    const MATERIAL: &str = r#"%YAML 1.1
--- !Material &1
name: tint_material
xshader: {localId: 1, path: xshader/tint.xshader}
properties:
  __class: PropertySheet
  floatmap: {__class: Map, intensity: 0.75, u_time: 0.0}
  vec2map: {__class: Map, u_center: {x: 0.5, y: 0.5}}
  vec3map: {__class: Map, u_tint: {x: 1.0, y: 0.55, z: 0.35}}
  vec4map: {__class: Map}
  intmap: {__class: Map, u_mode: 2}
  texmap:
    __class: Map
    inputImageTexture: {localId: 1, path: "share://input.texture"}
    u_gradientTexture: {localId: 1, path: image/gradient.png}
renderQueue: 3090
enabledMacros:
  __class: Map
  AE_DirLightNum: !<str> 0
  SAMPLETIIMES1: !<str> 40
"#;

    #[test]
    fn a_material_reads_its_seven_typed_uniform_maps() {
        let material = Material::parse("material/tint.material", MATERIAL.as_bytes()).unwrap();
        assert_eq!(material.floats["intensity"], 0.75);
        assert_eq!(material.ints["u_mode"], 2);
        assert_eq!(material.vec2["u_center"], [0.5, 0.5]);
        assert_eq!(material.vec3["u_tint"], [1.0, 0.55, 0.35]);
        assert_eq!(
            material.xshader.as_ref().unwrap().path.as_deref(),
            Some("xshader/tint.xshader")
        );
    }

    #[test]
    fn the_share_uri_is_what_binds_an_effect_to_the_compositors_frame() {
        // `share://input.texture` is the source video frame (§4.3). Treating
        // it as a file path is how an effect ends up sampling nothing.
        let material = Material::parse("material/tint.material", MATERIAL.as_bytes()).unwrap();
        assert_eq!(
            material.textures["inputImageTexture"],
            TextureSource::HostInput
        );
        assert_eq!(
            material.textures["u_gradientTexture"],
            TextureSource::Package("image/gradient.png".into())
        );
    }

    #[test]
    fn an_inline_str_tag_does_not_stop_the_parse() {
        // `!<str> 40` inside enabledMacros needs a resolver libyaml has no
        // reason to have, so the tag is stripped before parsing.
        let material = Material::parse("material/tint.material", MATERIAL.as_bytes()).unwrap();
        assert_eq!(material.macros["SAMPLETIIMES1"], "40");
    }

    const RT: &str = r#"%YAML 1.1
--- !ScreenRenderTexture &1
name: RTFilterOut
width: 720
height: 1280
internalFormat: {__class: InternalFormat, value: RGBA8}
filterMin: {__class: FilterMode, value: LINEAR}
wrapModeS: {__class: WrapMode, value: CLAMP}
shared: true
pecentX: 0.5
pecentY: 0.5
"#;

    #[test]
    fn a_render_target_scales_with_the_frame_rather_than_keeping_its_authored_size() {
        let rt = RenderTargetDesc::parse("rt/out.rt", RT.as_bytes()).unwrap();
        assert_eq!(rt.width, 720);
        assert!(rt.shared);
        // The authored size is 720x1280; at half scale on a 1080x1920 frame the
        // pass wants 540x960, not 720x1280.
        assert_eq!(rt.size_for((1080, 1920)), (540, 960));
    }

    const SCENE: &str = r#"%YAML 1.1
--- !Scene &1
name: tint_scene
entities:
  - __class: Entity
    name: Quad
    components:
      - {localId: 2}
      - __class: MeshRenderer
        sharedMaterials:
          - {localId: 1, path: material/tint.material}
        mesh: {localId: 1, path: mesh/quad.mesh}
      - __class: ScriptComponent
        enabled: true
        path: lua/TintScript.lua
        className: TintScript
        properties: {__class: Map, intensity: 0.75, speed: 2}
--- !Transform &2
localPosition: {x: 0, y: 0, z: 10}
"#;

    #[test]
    fn a_scene_yields_the_material_the_mesh_and_the_script_with_its_instance_defaults() {
        let entry = SceneEntry::parse("main.scene", SCENE.as_bytes()).unwrap();
        assert_eq!(entry.materials, ["material/tint.material"]);
        assert_eq!(entry.meshes, ["mesh/quad.mesh"]);
        assert_eq!(entry.scripts.len(), 1);

        let script = &entry.scripts[0];
        assert_eq!(script.path, "lua/TintScript.lua");
        assert_eq!(script.class_name, "TintScript");
        // §8.4: the scene's `properties` map overrides what `.new()` sets.
        assert_eq!(script.properties["intensity"].as_f64(), Some(0.75));
    }

    #[test]
    fn a_mesh_is_the_unit_quad_it_always_is() {
        let mesh = MeshAsset::parse(
            "mesh/quad.mesh",
            br#"%YAML 1.1
--- !Mesh &1
name: quad
vertices: [-1,-1,0, 0,0,  1,-1,0, 1,0,  1,1,0, 1,1,  -1,1,0, 0,1]
vertexAttribs:
  - semantic: {__class: VertexAttribType, value: POSITION}
  - semantic: {__class: VertexAttribType, value: TEXCOORD0}
submeshes:
  - indices16: [0, 1, 2, 2, 3, 0]
    indicesCount: 6
"#,
        )
        .unwrap();
        assert_eq!(mesh.vertices.len(), 20);
        assert_eq!(mesh.indices, [0, 1, 2, 2, 3, 0]);
        assert_eq!(mesh.attributes, ["POSITION", "TEXCOORD0"]);
    }
}
