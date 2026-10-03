//! The transition library: data-driven transitions beyond the five built-in
//! kinds, each one a WGSL module and a parameter table.
//!
//! Two sources:
//!
//! - **gl-transitions** (MIT, two BSD), ported to WGSL by `port.py` into
//!   `gl/*.wgsl` and the generated `gl_table.rs`. Licence texts and authors are
//!   in `LICENSE-gl-transitions.md`; each file keeps its own header.
//! - **Seamless short-form transitions** of our own (`seamless.wgsl`): zoom in
//!   and out through, spin, whip pan and push, with motion blur. They move
//!   both clips together, which works here because the compositor draws each
//!   side of a transition into a layer of its own — the thing a Resolve
//!   plugin cannot see (docs/research/resolve-plugins.md, 4.3 and 6.9).
//!
//! A library transition is a `TransitionMaterial` of kind `Library` whose
//! `preset` names one of these. Every preset shares one uniform layout:
//!
//! ```text
//! struct GlBlock { state: vec4<f32>, params: array<vec4<f32>, 12> }
//! state = (progress, ratio, direction, motion blur)
//! ```
//!
//! and the transition pipeline's layer bindings (outgoing, incoming,
//! sampler), so the compositor needs nothing new to draw one. Pipelines are
//! compiled the first time a preset is drawn and kept.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, OnceLock};

use parking_lot::Mutex;
use serde::Serialize;

use crate::modules::render::RenderContext;

mod gl_table;

/// How many `vec4` parameter slots every preset's uniform block has.
pub const SLOTS: usize = 12;

/// What kind of value a parameter holds, which decides its control.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GlParamKind {
    Float,
    Int,
    Bool,
    Vec2,
    Color3,
    Color4,
    IVec2,
}

impl GlParamKind {
    /// How many of the four floats in its slot the parameter uses.
    pub fn width(self) -> usize {
        match self {
            GlParamKind::Float | GlParamKind::Int | GlParamKind::Bool => 1,
            GlParamKind::Vec2 | GlParamKind::IVec2 => 2,
            GlParamKind::Color3 => 3,
            GlParamKind::Color4 => 4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct GlParam {
    pub name: &'static str,
    pub kind: GlParamKind,
    pub default: [f32; 4],
}

/// One row of the generated gl-transitions table.
#[derive(Debug, Clone, Copy)]
pub struct GlEntry {
    pub id: &'static str,
    pub upstream: &'static str,
    pub label: &'static str,
    pub author: &'static str,
    pub license: &'static str,
    pub wgsl: &'static str,
    pub params: &'static [GlParam],
}

/// Which family a preset belongs to, for the asset panel's categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Family {
    /// Ours: move both clips together, with motion blur.
    Seamless,
    /// Ported from gl-transitions.
    Gl,
}

/// One library transition.
#[derive(Debug, Clone, Serialize)]
pub struct Preset {
    /// `TransitionMaterial::preset`: `gl:<id>` or `seamless:<id>`. Never
    /// renamed once shipped.
    pub id: String,
    pub label: &'static str,
    pub family: Family,
    pub author: &'static str,
    pub license: &'static str,
    /// Whether the material's `direction` steers it.
    pub directional: bool,
    pub params: &'static [GlParam],
    #[serde(skip)]
    pub wgsl: &'static str,
    #[serde(skip)]
    pub entry: &'static str,
}

const SEAMLESS_WGSL: &str = include_str!("seamless.wgsl");

/// The seamless presets: id, label, entry point, directional, parameters.
const SEAMLESS: &[(&str, &str, &str, bool, &[GlParam])] = &[
    (
        "zoom_in",
        "Zoom in through",
        "fs_zoom_in",
        false,
        &[GlParam {
            name: "amount",
            kind: GlParamKind::Float,
            default: [3.0, 0.0, 0.0, 0.0],
        }],
    ),
    (
        "zoom_out",
        "Zoom out through",
        "fs_zoom_out",
        false,
        &[GlParam {
            name: "amount",
            kind: GlParamKind::Float,
            default: [3.0, 0.0, 0.0, 0.0],
        }],
    ),
    (
        "spin",
        "Spin",
        "fs_spin",
        false,
        &[GlParam {
            name: "turns",
            kind: GlParamKind::Float,
            default: [0.5, 0.0, 0.0, 0.0],
        }],
    ),
    ("whip", "Whip pan", "fs_whip", true, &[]),
    ("push", "Push", "fs_push", true, &[]),
];

/// Every library preset: the seamless ones first, then gl-transitions in
/// alphabetical order.
pub fn presets() -> &'static [Preset] {
    static PRESETS: OnceLock<Vec<Preset>> = OnceLock::new();
    PRESETS.get_or_init(|| {
        let seamless = SEAMLESS
            .iter()
            .map(|&(id, label, entry, directional, params)| Preset {
                id: format!("seamless:{id}"),
                label,
                family: Family::Seamless,
                author: "chukcut",
                license: "GPL-3.0-or-later",
                directional,
                params,
                wgsl: SEAMLESS_WGSL,
                entry,
            });
        let gl = gl_table::GL_TRANSITIONS.iter().map(|e| Preset {
            id: format!("gl:{}", e.id),
            label: e.label,
            family: Family::Gl,
            author: e.author,
            license: e.license,
            directional: false,
            params: e.params,
            wgsl: e.wgsl,
            entry: GL_ENTRY,
        });
        seamless.chain(gl).collect()
    })
}

/// A preset by id, with its index into [`presets`].
pub fn preset(id: &str) -> Option<(usize, &'static Preset)> {
    presets().iter().enumerate().find(|(_, p)| p.id == id)
}

/// The values a preset's uniform block gets: each parameter's default, with
/// the material's stored values over it. A stored value with the wrong number
/// of components is ignored rather than half applied.
pub fn values(preset: &Preset, stored: &BTreeMap<String, Vec<f32>>) -> [[f32; 4]; SLOTS] {
    let mut out = [[0.0; 4]; SLOTS];
    for (slot, param) in preset.params.iter().enumerate().take(SLOTS) {
        out[slot] = param.default;
        if let Some(v) = stored.get(param.name) {
            if v.len() == param.kind.width() && v.iter().all(|x| x.is_finite()) {
                out[slot][..v.len()].copy_from_slice(v);
            }
        }
    }
    out
}

/// The vertex stage every preset is compiled with: a fullscreen triangle with
/// a top-left UV, the input `port.py`'s harness and `seamless.wgsl` expect at
/// location 0.
const VERTEX_WGSL: &str = r#"
struct ChukcutLibraryVertex {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn chukcut_library_vs(@builtin(vertex_index) index: u32) -> ChukcutLibraryVertex {
    let x = f32((index << 1u) & 2u);
    let y = f32(index & 2u);
    var out: ChukcutLibraryVertex;
    out.position = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    out.uv = vec2<f32>(x, y);
    return out;
}
"#;

/// The entry point every gl-transitions preset is drawn with: see
/// [`GL_OUTPUT_WGSL`].
const GL_ENTRY: &str = "chukcut_main";

/// A second fragment entry for every ported gl-transition, which runs the
/// port's own `main` and premultiplies what it wrote.
///
/// The library pipeline blends with `One, OneMinusSrcAlpha` and wants
/// premultiplied colour: the sum is the same as straight alpha with
/// `SrcAlpha, OneMinusSrcAlpha`, but an 8-bit blender may round its factors
/// to the target's precision first, and NVIDIA rounds the source alpha of an
/// `Rgba8UnormSrgb` target to 1/255 (a fade from nothing at a progress of
/// 0.0018 drew black). Multiplying here keeps it in 32-bit float.
///
/// The port's `main` writes straight alpha, and WGSL cannot call an entry
/// point, so this calls what naga translated `main` into: `main_1`, reading
/// `v_uv_1` and writing `o_color`. `port.py`'s harness fixes those names for
/// every file in `gl/`, and `every_preset_is_valid_wgsl_with_the_shared_bindings`
/// fails if a regenerated port changes them.
const GL_OUTPUT_WGSL: &str = r#"
@fragment
fn chukcut_main(@location(0) v_uv: vec2<f32>) -> @location(0) vec4<f32> {
    v_uv_1 = v_uv;
    main_1();
    let c = o_color;
    return vec4<f32>(c.rgb * c.a, c.a);
}
"#;

/// The full source a preset compiles from.
pub fn source(preset: &Preset) -> String {
    let output = match preset.family {
        Family::Gl => GL_OUTPUT_WGSL,
        Family::Seamless => "",
    };
    format!("{}\n{}{}", preset.wgsl, VERTEX_WGSL, output)
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct LibraryUniform {
    pub state: [f32; 4],
    pub params: [[f32; 4]; SLOTS],
}

unsafe impl bytemuck::Zeroable for LibraryUniform {}
unsafe impl bytemuck::Pod for LibraryUniform {}

/// Pipelines per preset, compiled on first draw.
pub(crate) struct LibraryPipelines {
    uniform_layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    format: wgpu::TextureFormat,
    /// Compiled modules by the address of their source: the seamless presets
    /// share one.
    modules: Mutex<HashMap<usize, Arc<wgpu::ShaderModule>>>,
    pipelines: Mutex<HashMap<usize, Arc<wgpu::RenderPipeline>>>,
    pub(crate) stride: u32,
}

impl LibraryPipelines {
    /// `texture_layout` is the transition pipeline's own layer layout, so the
    /// bind group it makes for the two layers works here unchanged.
    pub(crate) fn new(
        ctx: &RenderContext,
        texture_layout: &wgpu::BindGroupLayout,
        format: wgpu::TextureFormat,
    ) -> Self {
        let device = ctx.device();
        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("chukcut library transition uniforms"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(
                        std::mem::size_of::<LibraryUniform>() as u64
                    ),
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("chukcut library transition layout"),
            bind_group_layouts: &[Some(&uniform_layout), Some(texture_layout)],
            immediate_size: 0,
        });
        let align = ctx.limits().min_uniform_buffer_offset_alignment.max(1) as u64;
        let stride = (std::mem::size_of::<LibraryUniform>() as u64).div_ceil(align) * align;
        Self {
            uniform_layout,
            pipeline_layout,
            format,
            modules: Mutex::new(HashMap::new()),
            pipelines: Mutex::new(HashMap::new()),
            stride: stride as u32,
        }
    }

    pub(crate) fn uniform_layout(&self) -> &wgpu::BindGroupLayout {
        &self.uniform_layout
    }

    /// The pipeline of preset `index`, compiled now if it never was.
    pub(crate) fn pipeline(&self, ctx: &RenderContext, index: usize) -> Arc<wgpu::RenderPipeline> {
        if let Some(hit) = self.pipelines.lock().get(&index) {
            return Arc::clone(hit);
        }
        let preset = &presets()[index];
        let device = ctx.device();
        let module = {
            let mut modules = self.modules.lock();
            Arc::clone(
                modules
                    .entry(preset.wgsl.as_ptr() as usize)
                    .or_insert_with(|| {
                        Arc::new(device.create_shader_module(wgpu::ShaderModuleDescriptor {
                            label: Some(&preset.id),
                            source: wgpu::ShaderSource::Wgsl(source(preset).into()),
                        }))
                    }),
            )
        };
        let pipeline = Arc::new(
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(&preset.id),
                layout: Some(&self.pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("chukcut_library_vs"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(preset.entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: self.format,
                        // Source-over, as every transition draws, with the
                        // colour premultiplied by the shader: `seamless.wgsl`
                        // returns it so, and a gl-transition is drawn through
                        // `GL_OUTPUT_WGSL`.
                        blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            }),
        );
        self.pipelines.lock().insert(index, Arc::clone(&pipeline));
        pipeline
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_preset_is_valid_wgsl_with_the_shared_bindings() {
        for preset in presets() {
            let source = source(preset);
            let module = naga::front::wgsl::parse_str(&source)
                .unwrap_or_else(|e| panic!("{}: {}", preset.id, e.emit_to_string(&source)));
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::default(),
            )
            .validate(&module)
            .unwrap_or_else(|e| panic!("{}: {e:?}", preset.id));
            assert!(
                module
                    .entry_points
                    .iter()
                    .any(|e| e.name == preset.entry && e.stage == naga::ShaderStage::Fragment),
                "{} has no fragment entry {}",
                preset.id,
                preset.entry
            );
        }
    }

    #[test]
    fn ids_are_unique_and_the_catalogue_is_whole() {
        let mut seen = std::collections::HashSet::new();
        for preset in presets() {
            assert!(seen.insert(preset.id.as_str()), "{} twice", preset.id);
            assert!(preset.params.len() <= SLOTS, "{}", preset.id);
        }
        assert_eq!(
            presets().iter().filter(|p| p.family == Family::Gl).count(),
            120
        );
        assert_eq!(
            presets()
                .iter()
                .filter(|p| p.family == Family::Seamless)
                .count(),
            5
        );
    }

    #[test]
    fn every_gl_preset_names_its_author_and_a_permissive_licence() {
        for preset in presets().iter().filter(|p| p.family == Family::Gl) {
            assert!(!preset.author.is_empty(), "{}", preset.id);
            assert!(
                matches!(preset.license, "MIT" | "BSD 2 Clause" | "BSD 3 Clause"),
                "{}: {}",
                preset.id,
                preset.license
            );
            assert!(
                preset
                    .wgsl
                    .contains(&format!("// License: {}", preset.license)),
                "{} lost its licence header",
                preset.id
            );
        }
    }

    #[test]
    fn stored_values_override_defaults_only_when_they_fit() {
        let (_, preset) = preset("gl:directional").expect("ported");
        let mut stored = BTreeMap::new();
        stored.insert("direction".to_string(), vec![-1.0, 0.0]);
        assert_eq!(values(preset, &stored)[0], [-1.0, 0.0, 0.0, 0.0]);
        stored.insert("direction".to_string(), vec![5.0]);
        assert_eq!(values(preset, &stored)[0], preset.params[0].default);
    }
}

#[cfg(test)]
mod gpu_tests;
