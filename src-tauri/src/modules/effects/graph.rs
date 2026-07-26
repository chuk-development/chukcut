//! The multi-pass render graph: a chain of full-screen passes over ping-ponged
//! render targets, driven by Lua.
//!
//! ## The chain
//!
//! ```text
//!   the composited frame  ──►  share://input.texture
//!                                    │
//!            ┌───────────────────────┴───────────────────────┐
//!            │  pass 0 → rt/midRT.rt   (declared by .xshader) │
//!            │  pass 1 → rt/midRT2.rt                         │
//!            │  pass N → the chain's output texture           │
//!            └────────────────────────────────────────────────┘
//!                                    │
//!                                    ▼
//!                         a pooled RGBA8 texture
//! ```
//!
//! A pass's target comes from the `.xshader` (§4.4: "A single `.xshader` can
//! declare multiple passes, each with its own render target and render state").
//! The last pass's target is redirected to the chain's output, because the
//! package's own final `.rt` is a name for "the screen" that only makes sense
//! inside their engine.
//!
//! ## Where the uniforms come from, in order
//!
//! §8.4 spells out the chain and this module implements exactly it:
//!
//! 1. the `.material`'s typed maps — the authored defaults;
//! 2. the script's scene-side `properties` — the instance's overrides;
//! 3. whatever the script wrote this frame through `material:setFloat(...)`.
//!
//! (1) is loaded once into the shared [`MaterialHandle`]; (2) is handed to the
//! script as `comp.properties`; (3) overwrites entries in the same handle every
//! frame. Reading the handle after driving the script therefore gives the
//! resolved value with no merge logic anywhere.
//!
//! ## What is not here
//!
//! Cameras, transforms and the entity hierarchy. A scene's cameras carry an
//! explicit `renderOrder` and a `layerVisibleMask` and are the *other* way a
//! multi-pass effect is expressed; this graph handles the `.xshader` way. See
//! `assets::SceneEntry` for what is read from a scene and what is not.

use std::collections::BTreeMap;
use std::sync::Arc;

use super::assets::{Material, Pass, RenderTargetDesc, SceneEntry, TextureSource, XShader};
use super::lua::{HostState, LuaRuntime, MaterialHandle, MaterialValues, ScriptInstance};
use super::package::{self, EffectPackage, Link};
use super::shader::{self, BindingLayout};
use crate::modules::render::{PooledTexture, RenderContext, TextureKey};

#[derive(Debug, thiserror::Error)]
pub enum EffectError {
    #[error(transparent)]
    Package(#[from] super::package::PackageError),
    #[error(transparent)]
    Asset(#[from] super::assets::AssetError),
    #[error(transparent)]
    Shader(#[from] super::shader::ShaderError),
    #[error(transparent)]
    Script(#[from] super::lua::ScriptError),
    #[error("{0}")]
    Unsupported(String),
}

pub type Result<T> = std::result::Result<T, EffectError>;

pub const OUTPUT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// What an effect target has to be able to do: be drawn into, be sampled by
/// the next pass, and be copied out. Deliberately the same set the compositor
/// gives its own target, so the chain's output is a drop-in replacement for it.
const TARGET_USAGE: wgpu::TextureUsages = wgpu::TextureUsages::RENDER_ATTACHMENT
    .union(wgpu::TextureUsages::TEXTURE_BINDING)
    .union(wgpu::TextureUsages::COPY_SRC);

/// Where one of a pass's samplers gets its pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
enum TextureSlot {
    /// `share://input.texture` — the frame the compositor just produced.
    Input,
    /// Another pass's target, by `.rt` path.
    Target(String),
    /// A still in the package.
    Still(String),
    /// Declared and unresolvable. Bound to a 1x1 transparent texel rather than
    /// left unbound, because WebGPU has no optional binding and an unbound
    /// slot is a validation error rather than a black sample.
    Missing(String),
}

struct GpuPass {
    name: String,
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    layout: BindingLayout,
    /// `(binding order, uniform name, slot)`.
    textures: Vec<(u32, Option<u32>, String, TextureSlot)>,
    uniform_buffer: Option<wgpu::Buffer>,
    /// `None` for the last pass, which writes the chain's output.
    target: Option<String>,
    clear: Option<wgpu::Color>,
}

pub struct EffectChain {
    ctx: Arc<RenderContext>,
    passes: Vec<GpuPass>,
    material: MaterialHandle,
    runtime: LuaRuntime,
    script: Option<ScriptInstance>,
    component: Option<mlua::Table>,
    targets: BTreeMap<String, RenderTargetDesc>,
    stills: BTreeMap<String, wgpu::Texture>,
    placeholder: wgpu::Texture,
    sampler: wgpu::Sampler,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    /// The chain's own texture pool, separate from the compositor's on purpose:
    /// an effect's intermediate targets are a different size distribution from
    /// a frame, and a chain that churns through them must never evict the
    /// compositor's frame textures.
    pool: crate::modules::render::TexturePool,
    started: bool,
}

/// The unit quad every corpus `.mesh` is, interleaved position(vec3) + uv(vec2).
///
/// V flipped against the `.mesh` in the corpus: their `.rt` targets are OpenGL
/// framebuffers with the origin at the bottom left and wgpu's is at the top
/// left, so an effect authored against theirs renders upside down unless the
/// quad compensates. This is the single most common way a ported effect looks
/// "nearly right".
const QUAD: [f32; 20] = [
    -1.0, -1.0, 0.0, 0.0, 1.0, //
    1.0, -1.0, 0.0, 1.0, 1.0, //
    1.0, 1.0, 0.0, 1.0, 0.0, //
    -1.0, 1.0, 0.0, 0.0, 0.0,
];
const QUAD_INDICES: [u16; 6] = [0, 1, 2, 2, 3, 0];

impl EffectChain {
    /// Build the chain for a package's first renderable link.
    pub fn load(ctx: Arc<RenderContext>, pkg: &EffectPackage) -> Result<Self> {
        let links = pkg.links();
        // `links()` returns them sorted by `zorder`, which is not the order
        // they appear in `config.json` — so the first *applied* link is what
        // this takes, and `link_index` below finds its position in the file.
        let link: &Link = links.first().copied().ok_or_else(|| {
            EffectError::Unsupported(
                "this package declares no links, so there is nothing to render. \
                 Model-only and Lynx-Studio packages are like this by design."
                    .into(),
            )
        })?;
        let link_index = pkg
            .config()
            .effect
            .as_ref()
            .map(|e| {
                e.link
                    .iter()
                    .position(|l| l.path == link.path)
                    .unwrap_or(0)
            })
            .unwrap_or(0);

        let base = link.path.clone();

        // The scene names the material and the script. Both forms of scene
        // root are handled by `scene_root`.
        let scene_path = pkg.scene_root(link_index).ok_or_else(|| {
            EffectError::Unsupported(format!(
                "{base} has neither a main.scene nor a content.json filemap prefab"
            ))
        })?;
        let scene = SceneEntry::parse(&scene_path, &pkg.read(&scene_path)?)?;

        let material_path = scene.materials.first().cloned().ok_or_else(|| {
            EffectError::Unsupported(format!("{scene_path} draws no material"))
        })?;
        let material_path = package::join_link(&base, &material_path);
        let material = Material::parse(&material_path, &pkg.read(&material_path)?)?;

        let xshader_ref = material.xshader.as_ref().and_then(|r| r.path.clone());
        let xshader_path = xshader_ref.ok_or_else(|| {
            EffectError::Unsupported(format!("{material_path} names no xshader"))
        })?;
        let xshader_path = package::join_link(&base, &xshader_path);
        let xshader = XShader::parse(&xshader_path, &pkg.read(&xshader_path)?)?;

        if xshader.passes.is_empty() {
            return Err(EffectError::Unsupported(format!(
                "{xshader_path} declares no passes"
            )));
        }

        // Every `.rt` any pass or texture binding mentions.
        let mut targets = BTreeMap::new();
        for pass in &xshader.passes {
            if let Some(path) = pass.render_texture.as_ref().and_then(|r| r.path.clone()) {
                let full = package::join_link(&base, &path);
                let desc = RenderTargetDesc::parse(&full, &pkg.read(&full)?)?;
                targets.insert(path, desc);
            }
        }
        for source in material.textures.values() {
            if let TextureSource::Package(path) = source {
                if path.ends_with(".rt") && !targets.contains_key(path) {
                    let full = package::join_link(&base, path);
                    let desc = RenderTargetDesc::parse(&full, &pkg.read(&full)?)?;
                    targets.insert(path.clone(), desc);
                }
            }
        }

        let device = ctx.device();
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("chukcut effect sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let mut stills = BTreeMap::new();
        for source in material.textures.values() {
            let TextureSource::Package(path) = source else {
                continue;
            };
            if path.ends_with(".rt") {
                continue;
            }
            let full = package::join_link(&base, path);
            match load_still(&ctx, pkg, &full) {
                Ok(texture) => {
                    stills.insert(path.clone(), texture);
                }
                Err(error) => {
                    // A missing still is a degraded effect, not a broken one.
                    tracing::warn!(%path, %error, "effect texture could not be loaded");
                }
            }
        }

        let last = xshader.passes.len() - 1;
        let mut passes = Vec::with_capacity(xshader.passes.len());
        for (index, pass) in xshader.passes.iter().enumerate() {
            passes.push(build_pass(
                &ctx,
                pkg,
                &base,
                pass,
                &material,
                index == last,
            )?);
        }

        // The Lua side. A package need not have a script — the material's
        // authored uniforms are then the whole story — so this is optional.
        let runtime = LuaRuntime::new()?;
        let values = material_values(&material);
        let handle = MaterialHandle::new(material.name.clone(), values);

        let (script, component) = match scene.scripts.first() {
            Some(desc) => {
                let path = package::join_link(&base, &desc.path);
                let source = pkg.read_to_string(&path)?;
                runtime.load_script(&path, &source)?;
                let instance = runtime.instantiate(&path, &desc.class_name)?;
                let properties = scene_properties(&runtime, desc)?;
                let component =
                    runtime.make_component("effect", Some(handle.clone()), properties)?;
                // §8.4: the scene's `properties` map overrides what `.new()`
                // set, so it is applied to the instance as well as handed over
                // as `comp.properties` — scripts read it both ways.
                for (key, value) in &desc.properties {
                    if let Ok(v) = to_lua(&runtime, value) {
                        let _ = instance.set(key, v);
                    }
                }
                (Some(instance), Some(component))
            }
            None => (None, None),
        };

        let placeholder = transparent_texel(&ctx);

        Ok(Self {
            ctx: Arc::clone(&ctx),
            passes,
            material: handle,
            runtime,
            script,
            component,
            targets,
            stills,
            placeholder,
            sampler,
            vertices: create_buffer(device, bytemuck::cast_slice(&QUAD), wgpu::BufferUsages::VERTEX),
            indices: create_buffer(
                device,
                bytemuck::cast_slice(&QUAD_INDICES),
                wgpu::BufferUsages::INDEX,
            ),
            pool: crate::modules::render::TexturePool::new(64 * 1024 * 1024),
            started: false,
        })
    }

    pub fn pass_names(&self) -> Vec<&str> {
        self.passes.iter().map(|p| p.name.as_str()).collect()
    }

    pub fn material(&self) -> &MaterialHandle {
        &self.material
    }

    /// Push a host slider change into the script, as `onEvent`.
    pub fn set_parameter(&self, key: &str, value: f64) -> Result<()> {
        let (Some(script), Some(component)) = (&self.script, &self.component) else {
            return Ok(());
        };
        let event = self.runtime.make_event(vec![
            mlua::Value::String(
                self.runtime
                    .lua()
                    .create_string(key)
                    .map_err(|e| super::lua::ScriptError::Lua {
                        script: "<event>".into(),
                        message: e.to_string(),
                    })?,
            ),
            mlua::Value::Number(value),
        ])?;
        script.on_event(component, &event)?;
        Ok(())
    }

    /// Render the chain over `input`, at `size`, for a point `seconds` into
    /// the effect. Returns a pooled texture the caller must release.
    pub fn render(
        &mut self,
        input: &wgpu::TextureView,
        size: (u32, u32),
        seconds: f32,
        delta_seconds: f32,
    ) -> Result<PooledTexture> {
        self.runtime.set_host(HostState {
            input_width: size.0,
            input_height: size.1,
            output_width: size.0,
            output_height: size.1,
            frame_timestamp: seconds,
            ..Default::default()
        });

        if let (Some(script), Some(component)) = (&self.script, &self.component) {
            if !self.started {
                script.on_start(component)?;
                self.started = true;
            }
            script.on_update(component, delta_seconds)?;
            // Scrubbing calls `seekToTime` directly. Scripts that drive
            // themselves from `onUpdate` define it and call it internally, so
            // calling it here too would double-advance them — hence only when
            // the script did not define `onUpdate`.
            if !script.has_hook("onUpdate") {
                script.seek_to_time(component, seconds)?;
            }
        }

        let values = self.material.snapshot();
        let device = self.ctx.device();
        let pool = &self.pool;

        // Every intermediate target this frame, acquired from the pool and
        // held until the whole chain has run.
        let mut intermediates: BTreeMap<String, PooledTexture> = BTreeMap::new();
        for (path, desc) in &self.targets {
            let (width, height) = desc.size_for(size);
            intermediates.insert(
                path.clone(),
                pool.acquire(device, TextureKey::new(width, height, OUTPUT_FORMAT, TARGET_USAGE)),
            );
        }

        let output = pool.acquire(device, TextureKey::new(size.0, size.1, OUTPUT_FORMAT, TARGET_USAGE));
        let placeholder_view = self.placeholder.create_view(&Default::default());

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("chukcut effect chain"),
        });

        for pass in &self.passes {
            if let (Some(buffer), Some(block)) = (&pass.uniform_buffer, &pass.layout.uniform_block)
            {
                let mut bytes = vec![0u8; block.size.max(4) as usize];
                write_uniforms(&mut bytes, block, &values);
                self.ctx.queue().write_buffer(buffer, 0, &bytes);
            }

            let mut entries: Vec<wgpu::BindGroupEntry> = Vec::new();
            let views: Vec<wgpu::TextureView> = pass
                .textures
                .iter()
                .map(|(_, _, _, slot)| match slot {
                    TextureSlot::Input => input.clone(),
                    TextureSlot::Target(path) => intermediates
                        .get(path)
                        .map(|t| t.view().clone())
                        .unwrap_or_else(|| placeholder_view.clone()),
                    TextureSlot::Still(path) => self
                        .stills
                        .get(path)
                        .map(|t| t.create_view(&Default::default()))
                        .unwrap_or_else(|| placeholder_view.clone()),
                    TextureSlot::Missing(_) => placeholder_view.clone(),
                })
                .collect();

            for (index, (texture_binding, sampler_binding, _, _)) in
                pass.textures.iter().enumerate()
            {
                entries.push(wgpu::BindGroupEntry {
                    binding: *texture_binding,
                    resource: wgpu::BindingResource::TextureView(&views[index]),
                });
                if let Some(sampler_binding) = sampler_binding {
                    entries.push(wgpu::BindGroupEntry {
                        binding: *sampler_binding,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    });
                }
            }
            if let (Some(buffer), Some(block)) = (&pass.uniform_buffer, &pass.layout.uniform_block)
            {
                entries.push(wgpu::BindGroupEntry {
                    binding: block.binding,
                    resource: buffer.as_entire_binding(),
                });
            }

            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("chukcut effect bindings"),
                layout: &pass.bind_group_layout,
                entries: &entries,
            });

            let target_view = match &pass.target {
                Some(path) => intermediates
                    .get(path)
                    .map(|t| t.view().clone())
                    .unwrap_or_else(|| output.view().clone()),
                None => output.view().clone(),
            };

            {
                let mut render = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some(&pass.name),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &target_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: match pass.clear {
                                Some(colour) => wgpu::LoadOp::Clear(colour),
                                None => wgpu::LoadOp::Load,
                            },
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
                render.set_pipeline(&pass.pipeline);
                render.set_bind_group(0, &bind_group, &[]);
                render.set_vertex_buffer(0, self.vertices.slice(..));
                render.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint16);
                render.draw_indexed(0..QUAD_INDICES.len() as u32, 0, 0..1);
            }
        }

        self.ctx.queue().submit(Some(encoder.finish()));

        for (_, texture) in intermediates {
            pool.release(texture);
        }
        Ok(output)
    }

}

fn create_buffer(device: &wgpu::Device, contents: &[u8], usage: wgpu::BufferUsages) -> wgpu::Buffer {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("chukcut effect geometry"),
        size: contents.len() as u64,
        usage: usage | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: true,
    });
    buffer
        .slice(..)
        .get_mapped_range_mut()
        .expect("a buffer mapped at creation is mappable")
        .copy_from_slice(contents);
    buffer.unmap();
    buffer
}

fn transparent_texel(ctx: &RenderContext) -> wgpu::Texture {
    let texture = ctx.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("chukcut effect placeholder"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: OUTPUT_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    ctx.queue().write_texture(
        texture.as_image_copy(),
        &[0u8; 4],
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4),
            rows_per_image: Some(1),
        },
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    texture
}

fn load_still(ctx: &RenderContext, pkg: &EffectPackage, path: &str) -> Result<wgpu::Texture> {
    let bytes = pkg.read(path)?;
    let image = image::load_from_memory(&bytes)
        .map_err(|e| EffectError::Unsupported(format!("{path}: {e}")))?
        .to_rgba8();
    let (width, height) = image.dimensions();
    let texture = ctx.device().create_texture(&wgpu::TextureDescriptor {
        label: Some(path),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: OUTPUT_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    ctx.queue().write_texture(
        texture.as_image_copy(),
        &image,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * 4),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    Ok(texture)
}

fn build_pass(
    ctx: &RenderContext,
    pkg: &EffectPackage,
    base: &str,
    pass: &Pass,
    material: &Material,
    is_last: bool,
) -> Result<GpuPass> {
    let vertex_source = pass
        .vertex
        .as_ref()
        .ok_or_else(|| EffectError::Unsupported(format!("pass {} has no vertex shader", pass.name)))?;
    let fragment_source = pass.fragment.as_ref().ok_or_else(|| {
        EffectError::Unsupported(format!("pass {} has no fragment shader", pass.name))
    })?;

    let vertex_path = package::join_link(base, &vertex_source.source_path);
    let fragment_path = package::join_link(base, &fragment_source.source_path);

    let vertex = shader::compile(
        &pkg.read_to_string(&vertex_path)?,
        super::glsl::Stage::Vertex,
        &vertex_path,
    )?;
    let fragment = shader::compile(
        &pkg.read_to_string(&fragment_path)?,
        super::glsl::Stage::Fragment,
        &fragment_path,
    )?;

    let device = ctx.device();
    let vertex_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(&vertex_path),
        source: wgpu::ShaderSource::Wgsl(vertex.wgsl.clone().into()),
    });
    let fragment_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(&fragment_path),
        source: wgpu::ShaderSource::Wgsl(fragment.wgsl.clone().into()),
    });

    // The fragment shader owns the interesting bindings. A vertex shader in
    // this corpus samples nothing and, at most, reads a matrix — and since
    // both stages were compiled independently their binding numbers are in
    // separate spaces, so only the fragment's are bound. A vertex shader with
    // its own uniforms would need `spirv-webgpu-transform::mirrorpatch`, which
    // is the documented tool for making two stages agree.
    let mut entries = Vec::new();
    let mut textures = Vec::new();
    for binding in &fragment.layout.textures {
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: binding.texture_binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        });
        if let Some(sampler) = binding.sampler_binding {
            entries.push(wgpu::BindGroupLayoutEntry {
                binding: sampler,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            });
        }
        let slot = match material.textures.get(&binding.name) {
            Some(TextureSource::HostInput) => TextureSlot::Input,
            Some(TextureSource::Package(path)) if path.ends_with(".rt") => {
                TextureSlot::Target(path.clone())
            }
            Some(TextureSource::Package(path)) => TextureSlot::Still(path.clone()),
            _ => TextureSlot::Missing(binding.name.clone()),
        };
        textures.push((
            binding.texture_binding,
            binding.sampler_binding,
            binding.name.clone(),
            slot,
        ));
    }

    let uniform_buffer = fragment.layout.uniform_block.as_ref().map(|block| {
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: block.binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        });
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("chukcut effect uniforms"),
            size: block.size.max(16) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    });

    let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("chukcut effect bind group layout"),
        entries: &entries,
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("chukcut effect pipeline layout"),
        bind_group_layouts: &[Some(&bind_group_layout)],
        immediate_size: 0,
    });

    // Vertex attributes are bound by *semantic*, not by name or by order.
    // §4.4's `semantics` map exists precisely so a loader does not have to
    // guess from `attPosition` vs `a_position` vs `position`.
    let mut attributes = Vec::new();
    for input in &vertex.vertex_inputs {
        // Three fallbacks, in decreasing order of trustworthiness. The last one
        // matters more than it looks: glslang does not always preserve an
        // attribute's name through to SPIR-V, and an unnamed attribute that
        // silently defaulted to the UV offset would put the quad's texture
        // coordinates in the position slot — a black frame, with nothing in any
        // log to say why.
        let semantic = pass
            .semantics
            .get(&input.name)
            .map(String::as_str)
            .or_else(|| (!input.name.is_empty()).then(|| guess_semantic(&input.name)))
            .unwrap_or(if input.location == 0 {
                "POSITION"
            } else {
                "TEXCOORD0"
            });
        let (offset, format) = match semantic {
            "POSITION" => (
                0,
                match input.components {
                    4 => wgpu::VertexFormat::Float32x3, // vec4 position from a vec3 buffer
                    n => size_format(n),
                },
            ),
            _ => (12, size_format(input.components.min(2))),
        };
        attributes.push(wgpu::VertexAttribute {
            format,
            offset,
            shader_location: input.location,
        });
    }

    let blend = pass.blend.enabled.then(|| wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: blend_factor(&pass.blend.src_color),
            dst_factor: blend_factor(&pass.blend.dst_color),
            operation: wgpu::BlendOperation::Add,
        },
        alpha: wgpu::BlendComponent {
            src_factor: blend_factor(&pass.blend.src_alpha),
            dst_factor: blend_factor(&pass.blend.dst_alpha),
            operation: wgpu::BlendOperation::Add,
        },
    });

    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(&pass.name),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &vertex_module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: 20,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &attributes,
            })],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &fragment_module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: OUTPUT_FORMAT,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    });

    let target = if is_last {
        None
    } else {
        pass.render_texture.as_ref().and_then(|r| r.path.clone())
    };

    Ok(GpuPass {
        name: pass.name.clone(),
        pipeline,
        bind_group_layout,
        layout: fragment.layout,
        textures,
        uniform_buffer,
        target,
        clear: pass.clear.then_some(wgpu::Color {
            r: pass.clear_color[0] as f64,
            g: pass.clear_color[1] as f64,
            b: pass.clear_color[2] as f64,
            a: pass.clear_color[3] as f64,
        }),
    })
}

fn size_format(components: u32) -> wgpu::VertexFormat {
    match components {
        1 => wgpu::VertexFormat::Float32,
        2 => wgpu::VertexFormat::Float32x2,
        3 => wgpu::VertexFormat::Float32x3,
        _ => wgpu::VertexFormat::Float32x4,
    }
}

/// When a pass declares no semantic for an attribute — older hand-authored
/// shaders often do not — fall back to the naming conventions the corpus uses.
fn guess_semantic(name: &str) -> &'static str {
    let lower = name.to_ascii_lowercase();
    if lower.contains("pos") {
        "POSITION"
    } else {
        "TEXCOORD0"
    }
}

fn blend_factor(name: &str) -> wgpu::BlendFactor {
    // Vulkan-style enum names (§4.4).
    match name {
        "ZERO" => wgpu::BlendFactor::Zero,
        "ONE" => wgpu::BlendFactor::One,
        "SRC_COLOR" => wgpu::BlendFactor::Src,
        "ONE_MINUS_SRC_COLOR" => wgpu::BlendFactor::OneMinusSrc,
        "DST_COLOR" => wgpu::BlendFactor::Dst,
        "ONE_MINUS_DST_COLOR" => wgpu::BlendFactor::OneMinusDst,
        "SRC_ALPHA" => wgpu::BlendFactor::SrcAlpha,
        "ONE_MINUS_SRC_ALPHA" => wgpu::BlendFactor::OneMinusSrcAlpha,
        "DST_ALPHA" => wgpu::BlendFactor::DstAlpha,
        "ONE_MINUS_DST_ALPHA" => wgpu::BlendFactor::OneMinusDstAlpha,
        _ => wgpu::BlendFactor::One,
    }
}

fn material_values(material: &Material) -> MaterialValues {
    MaterialValues {
        floats: material.floats.clone(),
        ints: material.ints.clone(),
        vec2: material.vec2.clone(),
        vec3: material.vec3.clone(),
        vec4: material.vec4.clone(),
        textures: BTreeMap::new(),
        macros: BTreeMap::new(),
    }
}

fn scene_properties(
    runtime: &LuaRuntime,
    desc: &super::assets::ScriptComponentDesc,
) -> Result<BTreeMap<String, mlua::Value>> {
    let mut out = BTreeMap::new();
    for (key, value) in &desc.properties {
        if let Ok(v) = to_lua(runtime, value) {
            out.insert(key.clone(), v);
        }
    }
    Ok(out)
}

fn to_lua(runtime: &LuaRuntime, value: &serde_yaml_ng::Value) -> mlua::Result<mlua::Value> {
    Ok(match value {
        serde_yaml_ng::Value::Bool(v) => mlua::Value::Boolean(*v),
        serde_yaml_ng::Value::Number(v) => match v.as_i64() {
            Some(i) => mlua::Value::Integer(i),
            None => mlua::Value::Number(v.as_f64().unwrap_or(0.0)),
        },
        serde_yaml_ng::Value::String(v) => {
            mlua::Value::String(runtime.lua().create_string(v)?)
        }
        _ => mlua::Value::Nil,
    })
}

/// Write the resolved uniform values into the block's std140 layout.
///
/// A value the shader does not declare is dropped, and a declared uniform the
/// material never set stays zero. Both are normal: a `.material` carries the
/// union of what every pass of its `.xshader` wants.
fn write_uniforms(bytes: &mut [u8], block: &super::shader::UniformBlock, values: &MaterialValues) {
    for member in &block.members {
        let offset = member.offset as usize;
        let write = |bytes: &mut [u8], data: &[u8]| {
            if offset + data.len() <= bytes.len() {
                bytes[offset..offset + data.len()].copy_from_slice(data);
            }
        };
        if let Some(v) = values.floats.get(&member.name) {
            write(bytes, &v.to_le_bytes());
        } else if let Some(v) = values.ints.get(&member.name) {
            // The GLSL type decides the encoding, not the Lua type: a script
            // that wrote an integral number into a `float` uniform through the
            // bracket shortcut must still produce a float in the buffer.
            if member.glsl_type == "float" {
                write(bytes, &(*v as f32).to_le_bytes());
            } else {
                write(bytes, &(*v as i32).to_le_bytes());
            }
        } else if let Some(v) = values.vec2.get(&member.name) {
            write(bytes, bytemuck::cast_slice(v));
        } else if let Some(v) = values.vec3.get(&member.name) {
            write(bytes, bytemuck::cast_slice(v));
        } else if let Some(v) = values.vec4.get(&member.name) {
            write(bytes, bytemuck::cast_slice(v));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::effects::shader::{UniformBlock, UniformMember};

    #[test]
    fn an_integer_written_into_a_float_uniform_is_encoded_as_a_float() {
        // The bracket shortcut dispatches on the *Lua* value's type, so a
        // script writing `material["u_intensity"] = 1` produces an int. The
        // shader declared a float, and the shader wins.
        let block = UniformBlock {
            group: 0,
            binding: 0,
            size: 8,
            members: vec![
                UniformMember {
                    name: "u_intensity".into(),
                    offset: 0,
                    size: 4,
                    glsl_type: "float".into(),
                },
                UniformMember {
                    name: "u_mode".into(),
                    offset: 4,
                    size: 4,
                    glsl_type: "int".into(),
                },
            ],
        };
        let mut values = MaterialValues::default();
        values.ints.insert("u_intensity".into(), 1);
        values.ints.insert("u_mode".into(), 3);

        let mut bytes = vec![0u8; 8];
        write_uniforms(&mut bytes, &block, &values);

        assert_eq!(f32::from_le_bytes(bytes[0..4].try_into().unwrap()), 1.0);
        assert_eq!(i32::from_le_bytes(bytes[4..8].try_into().unwrap()), 3);
    }

    #[test]
    fn a_uniform_the_material_never_set_stays_zero_rather_than_failing() {
        let block = UniformBlock {
            group: 0,
            binding: 0,
            size: 4,
            members: vec![UniformMember {
                name: "u_unset".into(),
                offset: 0,
                size: 4,
                glsl_type: "float".into(),
            }],
        };
        let mut bytes = vec![0xffu8; 4];
        write_uniforms(&mut bytes, &block, &MaterialValues::default());
        assert_eq!(bytes, [0xff, 0xff, 0xff, 0xff], "untouched, not zeroed");
    }

    // ---------------------------------------------------------------------
    // The end-to-end test: a package in their format, producing pixels.
    // ---------------------------------------------------------------------

    fn fixture() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/modules/effects/fixtures/tint")
    }

    /// Everything [`EffectChain::load`] does up to the point it needs a GPU:
    /// open the package, find the scene, follow it to the material, follow that
    /// to the `.xshader`, and resolve both passes' shader sources and targets.
    ///
    /// Separate from the render test so that a machine with no adapter still
    /// checks the whole resolution chain — which is where a format change would
    /// show up first.
    #[test]
    fn the_package_resolves_scene_to_material_to_xshader_to_both_passes() {
        let pkg = EffectPackage::open(fixture()).unwrap();

        let scene_path = pkg.scene_root(0).unwrap();
        assert_eq!(scene_path, "AmazingFeature/main.scene");
        let scene = SceneEntry::parse(&scene_path, &pkg.read(&scene_path).unwrap()).unwrap();
        assert_eq!(scene.materials, ["material/tint.material"]);
        assert_eq!(scene.scripts[0].class_name, "TintScript");
        assert_eq!(scene.scripts[0].properties["speed"].as_i64(), Some(2));

        let path = "AmazingFeature/material/tint.material";
        let material = Material::parse(path, &pkg.read(path).unwrap()).unwrap();
        assert_eq!(material.textures["inputImageTexture"], TextureSource::HostInput);
        assert_eq!(
            material.textures["desaturatedTexture"],
            TextureSource::Package("rt/desaturateRT.rt".into()),
        );

        let path = "AmazingFeature/xshader/tint.xshader";
        let xshader = XShader::parse(path, &pkg.read(path).unwrap()).unwrap();
        assert_eq!(xshader.passes.len(), 2);
        assert_eq!(
            xshader.passes[0].render_texture.as_ref().unwrap().path.as_deref(),
            Some("rt/desaturateRT.rt"),
        );
        assert_eq!(
            xshader.passes[0].fragment.as_ref().unwrap().source_path,
            "xshader/desaturate.frag",
        );
        // Both passes name the same vertex shader by `localId`, which is the
        // normal shape and the reason `!Shader` objects are separate documents
        // rather than being inlined into each pass.
        assert_eq!(
            xshader.passes[1].vertex.as_ref().unwrap().source_path,
            xshader.passes[0].vertex.as_ref().unwrap().source_path,
        );
        // The final pass names no target: it writes whatever the host gives it,
        // which is what the chain redirects to its output texture.
        assert!(xshader.passes[1].render_texture.is_none());
    }

    /// A solid-colour input frame, in the format and usage the compositor's own
    /// target has, so the chain is exercised exactly as it will be in place.
    fn solid_input(ctx: &RenderContext, size: (u32, u32), rgba: [u8; 4]) -> wgpu::Texture {
        let texture = ctx.device().create_texture(&wgpu::TextureDescriptor {
            label: Some("test input"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OUTPUT_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let row = size.0 as usize * 4;
        let mut data = Vec::with_capacity(row * size.1 as usize);
        for _ in 0..size.0 * size.1 {
            data.extend_from_slice(&rgba);
        }
        ctx.queue().write_texture(
            texture.as_image_copy(),
            &data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row as u32),
                rows_per_image: Some(size.1),
            },
            wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
        );
        texture
    }

    fn read_pixel(ctx: &RenderContext, texture: &PooledTexture) -> [u8; 4] {
        let (width, height) = (texture.width(), texture.height());
        let padded = (width * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = ctx.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("test readback"),
            size: (padded * height) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = ctx
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: texture.texture(),
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        ctx.queue().submit(Some(encoder.finish()));

        let slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        ctx.device().poll(wgpu::PollType::wait_indefinitely()).unwrap();
        rx.recv().unwrap().unwrap();
        let pixel = {
            let view = slice.get_mapped_range().unwrap();
            [view[0], view[1], view[2], view[3]]
        };
        buffer.unmap();
        pixel
    }

    /// The whole thing: a package written in CapCut's format is opened, its
    /// scene is read, its material's `.xshader` is compiled through the
    /// four-step pipeline, its Lua script drives the uniforms, and two GPU
    /// passes produce a frame.
    ///
    /// The assertion is directional rather than exact. What the effect does is
    /// pull a colour towards a desaturated, warm-tinted version of itself, so a
    /// pure red input must come out with *less* red and *more* green. Neither
    /// can happen by accident: a chain that failed to run gives back the input,
    /// and one that ran the wrong pass gives grey.
    #[test]
    fn one_effect_end_to_end_produces_pixels() {
        let Some(ctx) = crate::modules::render::test_context() else {
            eprintln!("no GPU adapter; skipping");
            return;
        };
        let package = EffectPackage::open(fixture()).unwrap();
        let mut chain = EffectChain::load(Arc::clone(&ctx), &package).unwrap();

        // Both passes were found, in the order the `.xshader` declares them.
        assert_eq!(chain.pass_names(), ["Desaturate", "Tint"]);

        let size = (64u32, 64u32);
        let input = solid_input(&ctx, size, [255, 0, 0, 255]);
        let view = input.create_view(&Default::default());

        // A quarter-turn of the script's own sine: `u_time = curTime * speed`
        // with `speed` = 2 from the scene's property map, so this puts the
        // effect at full strength.
        let delta = std::f32::consts::FRAC_PI_4;
        let output = chain.render(&view, size, delta, delta).unwrap();
        let pixel = read_pixel(&ctx, &output);

        assert!(
            pixel[0] < 250,
            "red should have been pulled down towards the desaturated value, got {pixel:?}"
        );
        assert!(
            pixel[1] > 5,
            "the warm tint should have introduced green, got {pixel:?}"
        );
        assert_eq!(pixel[3], 255, "alpha should be preserved");
    }

    /// The parameter chain of §8.4, end to end: the `.material` authored 0.8,
    /// the scene overrode the script instance to 0.8, and a host slider
    /// broadcast as `effects_adjust_intensity` overrides both.
    #[test]
    fn a_host_slider_changes_the_rendered_pixel() {
        let Some(ctx) = crate::modules::render::test_context() else {
            eprintln!("no GPU adapter; skipping");
            return;
        };
        let package = EffectPackage::open(fixture()).unwrap();
        let mut chain = EffectChain::load(Arc::clone(&ctx), &package).unwrap();

        let size = (64u32, 64u32);
        let input = solid_input(&ctx, size, [255, 0, 0, 255]);
        let view = input.create_view(&Default::default());
        let delta = std::f32::consts::FRAC_PI_4;

        let strong = read_pixel(&ctx, &chain.render(&view, size, delta, delta).unwrap());

        // Turn the effect off through the host's own mechanism.
        chain.set_parameter("effects_adjust_intensity", 0.0).unwrap();
        let off = read_pixel(&ctx, &chain.render(&view, size, delta, 0.0).unwrap());

        assert!(
            off[0] > strong[0],
            "at zero intensity the frame should be closer to the input, \
             got {off:?} against {strong:?}"
        );
    }

    #[test]
    fn vulkan_blend_factor_names_map_onto_wgpu() {
        assert_eq!(
            blend_factor("ONE_MINUS_SRC_ALPHA"),
            wgpu::BlendFactor::OneMinusSrcAlpha
        );
        assert_eq!(blend_factor("ONE"), wgpu::BlendFactor::One);
        // An unknown factor must not silently become Zero, which would make
        // the pass invisible.
        assert_eq!(blend_factor("SOMETHING_NEW"), wgpu::BlendFactor::One);
    }
}
