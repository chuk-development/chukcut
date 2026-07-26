# The effect runtime, and the state of the shader pipeline

**Status: an effect authored in CapCut's format renders.** 45 tests in
`modules/effects/`, including a two-pass chain over a real GPU that samples
`share://input.texture`, ping-pongs through a `.rt` target, and has its uniforms
driven from Lua. The fixture is `modules/effects/fixtures/tint/` and it was
written here — no ByteDance asset is in this repository.


Written 2026-07-26, while building `src-tauri/src/modules/effects/`. This is
the companion to [`effect-package-format.md`](effect-package-format.md), which
says what the format *is*; this one says what happens when you try to run it,
and which of the plans in the roadmap survived contact.

The short version: **the shader translation pipeline works, and it is the part
everyone expected to be the blocker.** What is not built is the binary asset
container, and that is now the thing standing between us and a downloaded
CapCut effect rendering.

---

## 1. The shader pipeline is proven, on real content

`rust-crate-survey.md` §6b proposed a four-step pipeline and closed by asking
for a conformance run over the corpus "before Phase 3 starts, not during it".
That run has now happened.

```
 GLSL ES 1.0 (no #version, precision highp float;)
        │  [1] our rewriter                 modules/effects/glsl.rs
        ▼
 GLSL 450 core
        │  [2] shaderc / glslang            --auto-bind-uniforms
        ▼                                   --auto-map-locations
 SPIR-V, combined image samplers
        │  [3] spirv-webgpu-transform 0.1.6
        ▼
 SPIR-V, separate texture + sampler
        │  [4] naga 30 spv-in
        ▼
 WGSL ──► wgpu::ShaderModule
```

**Result: 24 of 24.** Every `.vert` and `.frag` under `~/git/x` — twelve
fragment and twelve vertex shaders across `sample_effect`, `AmazingFeature` and
`effect2`, including four machine-generated `shaderLib/shaderGLES/<md5>` files
from the `.ausl` compiler — compiles to validated WGSL.

That is a small corpus and it should not be read as more than it is. It is,
however, *representative* in the ways the survey said mattered. Counted over
the same 24 files:

| feature | files | why it matters |
|---|---:|---|
| `sampler2D` as a **function parameter** | 1 | rules out a source-level sampler split; the case step [3] exists for |
| `gl_FragData[…]` | 8 | multiple colour attachments, one declared output per index written |
| arrays of samplers (`sampler2D x[N]`) | **0** | the one thing `spirv-webgpu-transform` documents as unsupported |
| `#version 300 es` | 0 | the ES 3.0 path is written but is untested against real content here |

Zero sampler arrays is the useful number: the known unsupported case does not
appear at all in this sample. That is not proof about the wider cache, but it
moves the risk from "unknown, possibly common" to "unknown, plausibly rare".

### What the run cost, in wall time

The whole conformance harness — writing the rewriter, running it, fixing the
two failures below — was about ninety minutes. The survey estimated "an
afternoon". It was right.

### Two dialect traps that only a run would find

Both were *compile errors*, never wrong pixels, which is the redeeming property
of this approach and worth stating plainly: when this pipeline is wrong, it
says so.

**`sample` is an identifier in ES 1.0 and a keyword in GLSL 450.** A Sobel
edge-detect shader in the corpus declares `vec3 sample;`, and 450 core reads
that as the interpolation qualifier:

```
pass1.frag:27: error: '' : syntax error, unexpected SAMPLE, expecting COMMA or SEMICOLON
```

There is a whole family of these — `layout`, `buffer`, `shared`, `uint`,
`smooth`, `centroid`, `patch`, `row_major` — every one of which is an ordinary
variable name in ES 1.0. The rewriter carries the list. It is deliberately only
the names ES 1.0 does *not* itself reserve: ES 1.0 already reserves `switch`,
`volatile`, `double` and the rest, so those can never appear as identifiers and
renaming them could only do harm.

**A shader may overload a builtin, and the obvious fix for the trap above
breaks it.** `LumiGrain` defines its own `float mix(float, float, float)` *and*
calls the builtin `mix(vec3, vec3, float)` elsewhere in the same file. Renaming
every occurrence of `mix` — which is what a naïve reserved-word pass does —
produced six errors of the form:

```
847a19bc…frag:98: error: 'mix_' : no matching overloaded function found
```

So the rename has to be driven by *declarations*, and a declaration whose name
is followed by `(` is a function. A function overloads; only a variable
shadows. `glsl.rs::rename_reserved` implements exactly that distinction and a
test pins it.

### Confirmed from the survey, by running it

- **Loose uniforms are illegal in Vulkan GLSL** and had to be wrapped in a
  block. This is the largest single piece of the rewriter and it is why
  `shader::UniformBlock` exists: the Lua side sets uniforms one at a time by
  name, so the runtime has to be able to find each one's byte offset again.
- **`--auto-map-bindings --auto-map-locations` solve the missing numbers.** The
  corpus declares no `layout(binding=…)` and no `layout(location=…)` anywhere.
- **Bindings are renumbered by the split.** `shader::layout_from` reconstructs
  the layout from the naga module *after* step [3] rather than from the GLSL
  before it. Reading it off the source would give pre-split numbers, which
  validate fine and bind the wrong texture.

### Corrections to the survey

- **`spirv-webgpu-transform`'s API is not what §6b describes.** The signature at
  0.1.6 is `combimgsampsplitter(&[u32], &mut Option<CorrectionMap>) ->
  Result<Vec<u32>, ()>` — it returns new words rather than mutating in place,
  and the corrections are an `Option` that it creates. The survey's
  `(&mut Vec<u32>, &mut CorrectionMap)` does not compile.
- **The `CorrectionMap` did not have to be interpreted at all.** The survey
  warns that it "must be read to build bind-group layouts". In practice the
  post-split naga module carries everything needed, and the texture↔sampler
  pairing is recoverable by ordering: the split emits each sampler immediately
  after the texture it came from. The map is still worth having as a check, and
  it is what a *vertex/fragment* binding disagreement would need.
- **`shaderc` rather than the `glslang` crate.** Ubuntu ships `libshaderc.so`
  and `shaderc-sys` links it; the build cost was seconds, not a CMake run. The
  `glslang` crate builds glslang from source. This is a Linux answer only —
  see the risk register below.

### What the run does *not* answer

- **Arrays of samplers.** `spirv-webgpu-transform`'s README says `sampler2D[N]`
  and `sampler2DArray[N]` are unsupported. No shader in this corpus uses one, so
  the question is still open on the wider cache.
- **A vertex shader with its own uniforms.** The two stages are compiled
  independently and therefore have independent binding spaces. Every corpus
  vertex shader is the same trivial pass-through, so this has not bitten; when
  it does, `spirv-webgpu-transform::mirrorpatch` is the documented tool for
  making two stages agree, and it is already in the dependency.
- **Whether the emitted WGSL is *correct*, as opposed to valid.** Everything
  above is compile-time. Nobody has yet compared a rendered frame against
  CapCut's own output for the same effect and parameters.
- **Whether `--auto-map-locations` agrees across the two stages.** The vertex
  and fragment shaders are compiled independently, so glslang assigns varying
  locations to each in isolation. With one varying — which is every corpus
  shader sampled — both get 0 and it works. A pair that declares its varyings
  in *different orders* would be mismatched, and wgpu would reject the pipeline
  rather than draw wrong, so this is loud too. Worth counting before it bites.

### The names do survive, which one thing depends on

A worry worth recording as settled: attribute names come through the whole
pipeline intact. `tint.vert`'s output is

```wgsl
@vertex
fn main(@location(1) attUV: vec2<f32>, @location(0) attPosition: vec3<f32>) -> VertexOutput
```

so `shader::VertexInput::name` is a real name and `graph.rs` can look the
attribute up in the `.xshader`'s `semantics` map, which is what that map is for
(§4.4). The fallback chain in `build_pass` exists anyway, because an unnamed
attribute defaulting to the UV offset would put texture coordinates in the
position slot — a black frame with nothing in any log to explain it.

---

## 2. What actually blocks a downloaded effect: the binary container

This is the honest headline of the whole exercise.

`effect-package-format.md` §4.1 establishes that structural assets ship in two
interchangeable encodings — the `%SerializedFormat%@` binary and a YAML 1.1
twin — and that the engine loads both. It presents the YAML as a gift, and it
is: it documents the object model exactly. But the survey's own distribution
table is the thing to read carefully:

| Extension | Binary | YAML |
|---|---:|---:|
| `.material` | 259 | 4 |
| `.xshader` | 209 | 4 |
| `.rt` | 186 | 24 |
| `.scene` | 56 | 2 |
| `.mesh` | 103 | 1 |

**About 2% of shipped assets are YAML.** The runtime reads the YAML twin and
refuses the binary by name. That is enough to run an effect authored in the
format — `modules/effects/fixtures/tint/` is one, and it renders — and it is
*not* enough to run anything downloaded. Every package in this cache would be
refused at its first `.xshader`.

So the ordering that looked right from the outside is inverted. The shader
pipeline was thought to be the risk and turned out to be a day's work with a
clean result. The binary container was written off as "safe to ignore" by the
2026-05 spec and is now the only thing between here and a real effect.

### What decoding it would take

Unmeasured, and this is an estimate rather than a finding. The container is
self-describing: `%SerializedFormat%@\n`, a 4-byte LE version (always 2), a
4-byte object count, then the objects. The YAML twin gives the complete field
list and type for every asset kind we care about, so this is not a
reverse-engineering problem in the usual sense — it is a "work out the type tag
encoding and the string table" problem, with an oracle for every answer.
The strongest possible position to be in.

The one lever worth trying first: `.xshader`, `.material` and `.rt` are the only
three that block rendering, and a hex dump of `sample_effect`'s `pass0.xshader`
already shows plaintext `gles2`, `attUV` and `attPosition` inline. Two days is
a plausible guess for those three. `.scene` is bigger and can be deferred,
because §2.2's prefab-rooted packages and the `.xshader` multi-pass form between
them cover a lot of effects without a scene graph.

---

## 3. The other stopping points, ranked

Everything below is known-missing rather than discovered-broken.

1. **The binary container.** Above. Blocks everything downloaded.
2. **The ECS scene graph.** `assets::SceneEntry` reads a scene for its material,
   its mesh and its script component with that script's property defaults, and
   nothing else. Cameras with an explicit `renderOrder` and `layerVisibleMask`
   are the *other* way a multi-pass effect is expressed — the `.xshader`
   multi-pass form is the one the graph implements. Prefab instancing,
   transforms and entity hierarchy are absent.
3. **The Lumi framework.** 110 sub-effect directories cache-wide are driven by
   `LumiManager`/`LumiObjectExtension`/`LumiParamsSetter`, which do pass
   ordering, ping-pong and parameter fan-out *in Lua* against APIs this runtime
   does not have (`Amaz.ScriptUtils.getLuaObj`, `Amaz.RenderTexture`,
   `scene:commitCommandBuffer`). This is Tier 2/3 of `LUA_API_SPEC.md` §9.
4. **JavaScript.** 91 `js-meta.json` against 109 `lua-meta.json`; the two
   colour-adjustment bundles are driven by a root `Feature.js`. Parameters are
   read from `js-meta.json` already, but there is no JS engine.
5. **The algorithm graph.** 183 packages carry an `algorithmConfig.json`. Face
   detection, matting and segmentation. Not a rendering problem and not close.
6. **AUSL.** Not needed: the generated GLSL ships alongside every `.ausl`.

---

## 4. Risks that cost money rather than time

- **`shaderc` on Windows and macOS is unverified.** On this machine it linked
  the system `libshaderc` in seconds. Elsewhere `shaderc-sys` falls back to
  building glslang and SPIRV-Tools from source with CMake and Python, which is
  slow but is a documented, supported path. If it fails outright, the
  alternatives are the `glslang` crate (same C++ build, different wrapper) or
  precompiling every shader — which the corpus's fixed content actually makes
  viable.
- **`spirv-webgpu-transform` has one maintainer and four stars.** It also has
  *no runtime dependencies at all*, so if it is abandoned the whole thing can be
  vendored. That is the reason it was acceptable.

---

## 5. The compositor patch

`render/` is not touched by this work, so the hook is written out here rather
than applied. It is small, and it is deliberately a trait: `render` must not
depend on Lua, on shader compilation or on zip reading, and an effect that
fails to load has to degrade to *no effect* rather than to *no frame*.

The good news first — **the composited target already carries
`TEXTURE_BINDING`**, because the NV12 compute pass needed it. So an effect can
sample the compositor's own output with no change to `TARGET_USAGE` and no
extra copy.

### 1. The trait, next to `TARGET_USAGE` in `compositor.rs`

```rust
/// A pass that runs over the composited frame before anything reads it.
///
/// The effect runtime implements this. A trait rather than a direct call for
/// two reasons: `render` must not depend on Lua, shader compilation or package
/// loading, and an effect that fails must degrade to "no effect" rather than to
/// "no frame" — which is what returning the input unchanged does.
///
/// Ownership of the texture passes through, so a pass that replaces the frame
/// is responsible for returning the old one to the pool and a pass that does
/// nothing is `|_, _, input, _, _| input`.
pub trait FramePass: Send + Sync {
    fn apply(
        &self,
        ctx: &Arc<RenderContext>,
        pool: &TexturePool,
        input: PooledTexture,
        size: (u32, u32),
        time: Micros,
    ) -> PooledTexture;
}
```

### 2. One field on `Compositor`

```rust
     stats: StatCounters,
+    /// Effect passes, in order, applied to every frame after compositing.
+    ///
+    /// A lock rather than a field because the chain changes when the document
+    /// does, and the compositor is shared between the preview's render loop
+    /// and the exporter's worker.
+    frame_passes: parking_lot::RwLock<Vec<Arc<dyn FramePass>>>,
     nv12: OnceLock<Option<Nv12Converter>>,
```

with `frame_passes: parking_lot::RwLock::new(Vec::new()),` in `with_config`,
and:

```rust
    /// Replace the effect chain. Cheap — it is a vector swap.
    pub fn set_frame_passes(&self, passes: Vec<Arc<dyn FramePass>>) {
        *self.frame_passes.write() = passes;
    }
```

### 3. Four lines at the end of `render_to_texture`

```rust
     add(&self.stats.composite_ns, composited);
     self.stats.frames.fetch_add(1, Ordering::Relaxed);

-    Ok(target)
+    // Effects run after compositing and before anything reads the frame, so
+    // the preview, the export and the NV12 path all get them without knowing
+    // they exist. `TARGET_USAGE` already carries `TEXTURE_BINDING`, so the
+    // composited texture is sampled where it lies.
+    let mut target = target;
+    for pass in self.frame_passes.read().iter() {
+        target = pass.apply(&self.ctx, &self.pool, target, size, time);
+    }
+    Ok(target)
 }
```

That is the whole change. Note where it sits: **inside** `render_to_texture`,
which `render`, `render_frame`, `render_nv12` and `render_nv12_into` all call,
so the preview and both export tiers pick effects up together and none of them
needs to know.

### The one thing the implementation side still needs

`EffectChain::render` takes `&mut self`, because `started` tracks whether
`onStart` has run. A `FramePass` gets `&self`. So the adapter holds the chain
behind a `Mutex` — which it wants anyway, because two frames must not drive the
same script concurrently.

The `Send` half of that is already paid for: `mlua`'s **`send` feature is
enabled in `Cargo.toml` and is load-bearing**, not a nicety. Without it `Lua`
and every handle it produces are `!Send`, and a `Mutex` does not rescue that —
`Mutex<T>` is only `Send` if `T` is. There is a test pinning it.

---

## 6. Reproducing the conformance run

The harness is `modules/effects/shader.rs`'s tests plus, for the corpus itself,
a throwaway binary. To re-run it over any set of shaders:

```rust
for path in paths {
    let stage = if path.ends_with(".vert") { Stage::Vertex } else { Stage::Fragment };
    match effects::shader::compile(&std::fs::read_to_string(&path)?, stage, &path) {
        Ok(_) => ok += 1,
        Err(e) => { println!("FAIL {path}\n{e}"); failed += 1 }
    }
}
```

Do this again against a wider cache before trusting the 24/24. The failures are
loud and each one is a bounded fix; the number that matters is what fraction
need a fix at all.
