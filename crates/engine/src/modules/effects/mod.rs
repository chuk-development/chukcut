//! Effect runtime.
//!
//! Loads a CapCut-format effect package at runtime and renders it as a chain of
//! full-screen GPU passes over the composited frame. Nothing ByteDance-authored
//! is in this repository or in a build: a package arrives from a directory or a
//! zip the user names, and everything under `fixtures/` was written here.
//!
//! ## The four layers, and where each one lives
//!
//! ```text
//!   a directory or a .zip                     package.rs
//!         │  config.json → effect.Link[]      (manifest, parameters)
//!         │  <Link>/content.json
//!         │  <Link>/lua-meta.json  ──────────►  Parameter[]  → the UI
//!         ▼
//!   assets: .xshader .material .rt .mesh      assets.rs
//!         │  render state, uniform values,
//!         │  texture bindings, pass targets
//!         ▼
//!   GLSL ES 1.0  ──►  WGSL + BindingLayout    glsl.rs, shader.rs
//!         │
//!         ▼
//!   a chain of passes over ping-ponged        graph.rs
//!   render targets, ordered and               lua.rs
//!   parameterised by Lua
//! ```
//!
//! ## What is proven and what is not
//!
//! The shader pipeline is the part that had to be proven before anything else
//! was worth writing, because the roadmap's plan for it does not work — see
//! `docs/research/rust-crate-survey.md` §6b. It now compiles **24 of 24** real
//! corpus shaders end to end; `glsl.rs` carries the two dialect traps that run
//! found, both of which are silent-wrong-answer material rather than obvious.
//!
//! What is **not** built: the binary `%SerializedFormat%@` container. Assets
//! ship in two interchangeable encodings and only the YAML twin is read here.
//! A package whose `.xshader` is binary is rejected with a message that says
//! so, rather than being half-parsed. See `assets.rs`.
//!
//! ## Legal boundary
//!
//! Reading their format is interoperability. Redistributing their content is
//! not. No package, shader, texture or manifest authored by ByteDance is
//! committed here; `fixtures/` contains one effect written for this test suite.

pub mod assets;
pub mod commands;
pub mod glsl;
pub mod graph;
pub mod lua;
pub mod package;
pub mod shader;

pub use assets::{AssetError, Material, MeshAsset, Pass, RenderTargetDesc, XShader};
pub use glsl::{rewrite, Rewritten, Stage, UniformDecl, UniformKind};
pub use graph::{EffectChain, EffectError};
pub use lua::{LuaRuntime, ScriptError};
pub use package::{
    EffectPackage, Link, PackageError, Parameter, ParameterKind, ParameterRange, Widget,
};
pub use shader::{BindingLayout, CompiledShader, ShaderError, TextureBinding, UniformMember};
