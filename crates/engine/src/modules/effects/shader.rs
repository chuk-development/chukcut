//! Steps [2] to [4]: GLSL 450 → SPIR-V → WebGPU-safe SPIR-V → WGSL.
//!
//! ```text
//!   GLSL 450 core (from glsl.rs)
//!        │  [2] shaderc / glslang, --auto-bind-uniforms --auto-map-locations
//!        ▼
//!   SPIR-V, still with combined image samplers
//!        │  [3] spirv-webgpu-transform: split them
//!        ▼
//!   SPIR-V with separate texture + sampler, plus a CorrectionMap
//!        │  [4] naga spv-in — a first-class frontend, unlike glsl-in
//!        ▼
//!   WGSL  ──►  wgpu::ShaderModule
//! ```
//!
//! ## Why step [3] exists at all
//!
//! WebGPU has no combined image samplers, and every CapCut shader declares
//! `uniform sampler2D`. Both naga frontends fail on one: glsl-in with
//! *"Not implemented: variable qualifier"* and spv-in with *"invalid id %59"*.
//! Splitting them in the SPIR-V rather than textually in the GLSL is what makes
//! this tractable — the corpus passes samplers as *function parameters*
//! (`vec4 gaussianBlur(sampler2D tex, …)`), and rewriting that in source means
//! doing type inference on GLSL. `spirv-webgpu-transform` does it in bytecode,
//! through function parameters and nesting, which is why it is worth a
//! bus-factor-one dependency. It has no runtime dependencies of its own, so
//! vendoring it later is a contained decision.
//!
//! ## Binding numbers move, and that is the part that bites
//!
//! The split renumbers bindings: source 0/1/2 becomes texture\@0, sampler\@1,
//! texture\@2, sampler\@3, block\@4. [`BindingLayout`] is reconstructed from
//! the naga module *after* the split rather than from the GLSL before it, so
//! the numbers it reports are the ones wgpu will see.

use super::glsl::{self, Stage, UniformKind};

#[derive(Debug, thiserror::Error)]
pub enum ShaderError {
    #[error("{name}: glslang rejected the rewritten source: {message}")]
    Glslang { name: String, message: String },
    #[error("{name}: the combined-sampler split failed")]
    Split { name: String },
    #[error("{name}: naga could not read the SPIR-V: {message}")]
    SpirV { name: String, message: String },
    #[error("{name}: the shader is not valid: {message}")]
    Validation { name: String, message: String },
    #[error("{name}: could not write WGSL: {message}")]
    Wgsl { name: String, message: String },
}

pub type Result<T> = std::result::Result<T, ShaderError>;

/// One texture the shader samples, and the sampler that step [3] paired it
/// with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextureBinding {
    /// The name the shader declared, e.g. `u_inputTexture`. This is the key a
    /// `.material`'s `texmap` uses.
    pub name: String,
    pub group: u32,
    pub texture_binding: u32,
    /// `None` when the split did not pair one, which would mean the shader
    /// only did `textureSize`-style queries.
    pub sampler_binding: Option<u32>,
}

/// One member of the synthesised uniform block, with the offset the runtime
/// writes at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UniformMember {
    pub name: String,
    pub offset: u32,
    pub size: u32,
    /// The GLSL type the source declared, kept so the Lua binding can refuse a
    /// `setFloat` into a `vec4`.
    pub glsl_type: String,
}

#[derive(Debug, Clone, Default)]
pub struct BindingLayout {
    pub textures: Vec<TextureBinding>,
    /// `None` when the shader declared no scalar uniforms at all.
    pub uniform_block: Option<UniformBlock>,
}

#[derive(Debug, Clone)]
pub struct UniformBlock {
    pub group: u32,
    pub binding: u32,
    pub size: u32,
    pub members: Vec<UniformMember>,
}

impl BindingLayout {
    pub fn texture(&self, name: &str) -> Option<&TextureBinding> {
        self.textures.iter().find(|t| t.name == name)
    }

    pub fn member(&self, name: &str) -> Option<&UniformMember> {
        self.uniform_block
            .as_ref()?
            .members
            .iter()
            .find(|m| m.name == name)
    }
}

/// One vertex attribute, with the location glslang's `--auto-map-locations`
/// gave it. Read off the compiled module rather than assumed, because the
/// order attributes are *declared* in is not guaranteed to be the order they
/// are *numbered* in, and a silent mismatch draws garbage rather than failing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VertexInput {
    pub name: String,
    pub location: u32,
    /// Component count, 1–4.
    pub components: u32,
}

#[derive(Debug, Clone)]
pub struct CompiledShader {
    pub name: String,
    pub stage: Stage,
    pub wgsl: String,
    pub layout: BindingLayout,
    /// Empty for a fragment shader.
    pub vertex_inputs: Vec<VertexInput>,
    /// The 450-core source, kept because a validation failure downstream is
    /// nearly unreadable without it.
    pub rewritten: String,
    /// The fragment shader writes premultiplied colour
    /// ([`glsl::premultiply_output`]); its pipeline must blend with `ONE`.
    pub premultiplied: bool,
}

/// Compile one GLSL ES shader all the way to WGSL.
pub fn compile(source: &str, stage: Stage, name: &str) -> Result<CompiledShader> {
    compile_with(source, stage, name, false)
}

/// [`compile`], and with `premultiply` a fragment shader that writes
/// premultiplied colour when it has an output to change. `premultiplied`
/// on the result says whether it does.
pub fn compile_with(
    source: &str,
    stage: Stage,
    name: &str,
    premultiply: bool,
) -> Result<CompiledShader> {
    let mut rewritten = glsl::rewrite(source, stage);
    let mut premultiplied = false;
    if premultiply && stage == Stage::Fragment {
        if let Some(wrapped) = glsl::premultiply_output(&rewritten.source) {
            rewritten.source = wrapped;
            premultiplied = true;
        }
    }
    let spirv = to_spirv(&rewritten.source, stage, name)?;

    let mut corrections: Option<spirv_webgpu_transform::CorrectionMap> = None;
    let split =
        spirv_webgpu_transform::combimgsampsplitter(&spirv, &mut corrections).map_err(|_| {
            ShaderError::Split {
                name: name.to_string(),
            }
        })?;

    let module =
        naga::front::spv::parse_u8_slice(as_bytes(&split), &naga::front::spv::Options::default())
            .map_err(|e| ShaderError::SpirV {
            name: name.to_string(),
            message: format!("{e:?}"),
        })?;

    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .map_err(|e| ShaderError::Validation {
        name: name.to_string(),
        message: format!("{e:?}"),
    })?;

    let wgsl =
        naga::back::wgsl::write_string(&module, &info, naga::back::wgsl::WriterFlags::empty())
            .map_err(|e| ShaderError::Wgsl {
                name: name.to_string(),
                message: format!("{e:?}"),
            })?;

    let layout = layout_from(&module, &rewritten);
    let vertex_inputs = match stage {
        Stage::Vertex => vertex_inputs_from(&module),
        Stage::Fragment => Vec::new(),
    };

    Ok(CompiledShader {
        name: name.to_string(),
        stage,
        wgsl,
        layout,
        vertex_inputs,
        rewritten: rewritten.source,
        premultiplied,
    })
}

fn vertex_inputs_from(module: &naga::Module) -> Vec<VertexInput> {
    let Some(entry) = module.entry_points.first() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for argument in &entry.function.arguments {
        let Some(naga::Binding::Location { location, .. }) = argument.binding else {
            continue;
        };
        let components = match &module.types[argument.ty].inner {
            naga::TypeInner::Scalar(_) => 1,
            naga::TypeInner::Vector { size, .. } => *size as u32,
            _ => continue,
        };
        out.push(VertexInput {
            name: argument.name.clone().unwrap_or_default(),
            location,
            components,
        });
    }
    out.sort_by_key(|input| input.location);
    out
}

fn to_spirv(source: &str, stage: Stage, name: &str) -> Result<Vec<u32>> {
    let compiler = shaderc::Compiler::new().map_err(|e| ShaderError::Glslang {
        name: name.to_string(),
        message: e.to_string(),
    })?;
    let mut options = shaderc::CompileOptions::new().map_err(|e| ShaderError::Glslang {
        name: name.to_string(),
        message: e.to_string(),
    })?;
    // The corpus declares no bindings and no locations anywhere, so both have
    // to be invented. This is the `--auto-map-bindings --auto-map-locations`
    // pair from the research document, through the library rather than the CLI.
    options.set_auto_bind_uniforms(true);
    options.set_auto_map_locations(true);
    options.set_target_env(
        shaderc::TargetEnv::Vulkan,
        shaderc::EnvVersion::Vulkan1_1 as u32,
    );

    let kind = match stage {
        Stage::Vertex => shaderc::ShaderKind::Vertex,
        Stage::Fragment => shaderc::ShaderKind::Fragment,
    };
    let artifact = compiler
        .compile_into_spirv(source, kind, name, "main", Some(&options))
        .map_err(|e| ShaderError::Glslang {
            name: name.to_string(),
            message: e.to_string(),
        })?;
    Ok(artifact.as_binary().to_vec())
}

fn as_bytes(words: &[u32]) -> &[u8] {
    // SPIR-V is little-endian words and every target we build for is
    // little-endian; naga's byte-slice entry point re-reads the magic number
    // and would reject a wrongly-ordered module rather than mis-parse it.
    unsafe { std::slice::from_raw_parts(words.as_ptr().cast::<u8>(), words.len() * 4) }
}

/// Rebuild the binding layout from the module the split produced, not from the
/// source. Reading it off the GLSL would give the pre-split numbers, which is
/// the mistake this whole function exists to avoid.
fn layout_from(module: &naga::Module, rewritten: &glsl::Rewritten) -> BindingLayout {
    use naga::TypeInner;

    let mut textures: Vec<(u32, u32, String)> = Vec::new();
    let mut samplers: Vec<(u32, u32)> = Vec::new();
    let mut uniform_block = None;

    for (_, variable) in module.global_variables.iter() {
        let Some(binding) = variable.binding.as_ref() else {
            continue;
        };
        let ty = &module.types[variable.ty];
        match &ty.inner {
            TypeInner::Image { .. } => {
                let name = variable.name.clone().unwrap_or_default();
                textures.push((binding.group, binding.binding, name));
            }
            TypeInner::Sampler { .. } => samplers.push((binding.group, binding.binding)),
            TypeInner::Struct { members, span } => {
                if variable.space != naga::AddressSpace::Uniform {
                    continue;
                }
                let fields = members
                    .iter()
                    .map(|m| {
                        let name = m.name.clone().unwrap_or_default();
                        let glsl_type = rewritten
                            .scalars()
                            .find(|u| u.name == name)
                            .and_then(|u| match &u.kind {
                                UniformKind::Scalar { glsl_type, .. } => Some(glsl_type.clone()),
                                _ => None,
                            })
                            .unwrap_or_default();
                        UniformMember {
                            name,
                            offset: m.offset,
                            size: module.types[m.ty].inner.size(module.to_ctx()),
                            glsl_type,
                        }
                    })
                    .collect();
                uniform_block = Some(UniformBlock {
                    group: binding.group,
                    binding: binding.binding,
                    size: *span,
                    members: fields,
                });
            }
            _ => {}
        }
    }

    textures.sort_by_key(|(group, binding, _)| (*group, *binding));
    samplers.sort();

    // The split emits each sampler immediately after the texture it was paired
    // with, so pairing by "the next sampler at a higher binding in the same
    // group" reconstructs the association without having to interpret the
    // CorrectionMap's shape.
    let textures = textures
        .into_iter()
        .map(|(group, texture_binding, name)| {
            let sampler_binding = samplers
                .iter()
                .filter(|(g, b)| *g == group && *b > texture_binding)
                .map(|(_, b)| *b)
                .min();
            TextureBinding {
                name,
                group,
                texture_binding,
                sampler_binding,
            }
        })
        .collect();

    BindingLayout {
        textures,
        uniform_block,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Written here, in the dialect the corpus uses: no `#version`, a
    /// `precision` statement, `varying`, `texture2D`, `gl_FragColor`, loose
    /// uniforms, and — the case that matters — a sampler passed as a function
    /// parameter, which is what breaks every source-level rewrite.
    const CORPUS_SHAPED: &str = r#"
precision highp float;
varying vec2 uv0;

uniform sampler2D inputImageTexture;
uniform sampler2D maskTexture;
uniform float intensity;
uniform vec2 blurStep;

vec4 blurred(sampler2D tex, vec2 uv, vec2 step) {
    vec4 sum = texture2D(tex, uv);
    for (int i = 1; i <= 2; i++) {
        sum += texture2D(tex, uv + float(i) * step);
        sum += texture2D(tex, uv - float(i) * step);
    }
    return sum / 5.0;
}

void main() {
    vec4 base = blurred(inputImageTexture, uv0, blurStep);
    float mask = texture2D(maskTexture, uv0).r;
    gl_FragColor = mix(base, base * intensity, mask);
}
"#;

    #[test]
    fn compiles_a_corpus_shaped_es1_fragment_shader_to_wgsl() {
        let out = compile(CORPUS_SHAPED, Stage::Fragment, "corpus_shaped.frag")
            .expect("the four-step pipeline");
        assert!(out.wgsl.contains("fn main"));
        // The whole point of step [3]: no combined sampler survives.
        assert!(out.wgsl.contains("texture_2d<f32>"));
        assert!(out.wgsl.contains("sampler"));
    }

    #[test]
    fn a_sampler_passed_as_a_function_parameter_survives_the_split() {
        // This is the case a textual GLSL rewrite cannot do without type
        // inference, and the reason the split happens in SPIR-V.
        let out = compile(CORPUS_SHAPED, Stage::Fragment, "corpus_shaped.frag").unwrap();
        assert_eq!(out.layout.textures.len(), 2);
    }

    #[test]
    fn the_reported_bindings_are_the_post_split_ones() {
        let out = compile(CORPUS_SHAPED, Stage::Fragment, "corpus_shaped.frag").unwrap();
        let input = out
            .layout
            .texture("inputImageTexture")
            .expect("named after the source declaration, not after a mangled id");
        let mask = out.layout.texture("maskTexture").unwrap();

        // Each texture got its own sampler, and no two share a binding number.
        assert!(input.sampler_binding.is_some());
        assert!(mask.sampler_binding.is_some());
        let mut used = vec![
            input.texture_binding,
            input.sampler_binding.unwrap(),
            mask.texture_binding,
            mask.sampler_binding.unwrap(),
        ];
        used.sort_unstable();
        used.dedup();
        assert_eq!(used.len(), 4, "bindings collided after the split");
    }

    #[test]
    fn scalar_uniforms_come_back_as_a_block_with_offsets() {
        let out = compile(CORPUS_SHAPED, Stage::Fragment, "corpus_shaped.frag").unwrap();
        let block = out
            .layout
            .uniform_block
            .as_ref()
            .expect("two loose uniforms");
        assert_eq!(block.members.len(), 2);

        let intensity = out.layout.member("intensity").unwrap();
        assert_eq!(intensity.glsl_type, "float");
        assert_eq!(intensity.size, 4);

        let step = out.layout.member("blurStep").unwrap();
        assert_eq!(step.glsl_type, "vec2");
        assert_eq!(step.size, 8);
        // std140: a vec2 aligns to 8, so it cannot follow a float at offset 4.
        assert_eq!(step.offset % 8, 0);
        assert!(step.offset > intensity.offset);
    }

    #[test]
    fn a_vertex_shader_in_the_es1_dialect_compiles_too() {
        let source = concat!(
            "attribute vec4 attPosition;\n",
            "attribute vec2 attUV;\n",
            "varying vec2 uv0;\n",
            "uniform mat4 u_mvp;\n",
            "void main() {\n",
            "  uv0 = attUV;\n",
            "  gl_Position = u_mvp * attPosition;\n",
            "}\n",
        );
        let out = compile(source, Stage::Vertex, "quad.vert").unwrap();
        assert!(out.wgsl.contains("fn main"));
        assert!(out.layout.uniform_block.is_some());
    }

    #[test]
    fn a_broken_shader_fails_at_compile_time_with_the_rewritten_source_named() {
        let error = compile("void main() { nonsense; }", Stage::Fragment, "bad.frag")
            .expect_err("glslang should refuse this");
        assert!(matches!(error, ShaderError::Glslang { .. }));
        assert!(error.to_string().contains("bad.frag"));
    }
}
