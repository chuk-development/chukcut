//! GLSL ES 1.0 → Vulkan-flavoured GLSL 450, step [1] of the shader pipeline.
//!
//! `docs/research/rust-crate-survey.md` §6b establishes why this file has to
//! exist: naga's GLSL frontend accepts only versions 440/450/460 and rejects
//! the `es` profile outright, and 224 of 228 shaders in the corpus carry no
//! `#version` line at all. glslang will not take ES 1.0 either — it wants 310
//! or higher for SPIR-V. So the source is rewritten into a dialect both tools
//! accept, and the sampler problem is solved later, in the SPIR-V, where it is
//! much easier.
//!
//! ## What the rewrite does
//!
//! | ES 1.0 | 450 core |
//! |---|---|
//! | *(no version line)* | `#version 450 core` |
//! | `attribute` | `in` |
//! | `varying` | `in` in a fragment shader, `out` in a vertex shader |
//! | `gl_FragColor` | a declared `layout(location = 0) out vec4` |
//! | `gl_FragData[N]` | one declared output per index actually written |
//! | `texture2D`, `textureCube` | `texture` |
//! | `uniform float x;` | a member of one `ChukcutUniforms` block |
//! | `uniform sampler2D t;` | left loose, for glslang to number |
//!
//! The uniform block is the largest piece, and it is not optional: Vulkan GLSL
//! rejects a loose non-opaque uniform outright — *"non-opaque uniforms outside
//! a block: not allowed when using GLSL for Vulkan"* — and nearly every corpus
//! shader has several, because the Lua side sets them one at a time through
//! `material:setFloat(...)`. [`Rewritten::uniforms`] is what lets the runtime
//! find each one's offset again.
//!
//! ## The two traps, both found by running the corpus rather than by reading it
//!
//! **`sample` is an identifier in ES 1.0 and a keyword in 450 core.** A Sobel
//! filter in the corpus declares `vec3 sample;` and 450 reads it as the
//! interpolation qualifier. There is a family of these — `layout`, `buffer`,
//! `shared`, `uint`, `smooth`, `centroid` — every one of which is a perfectly
//! ordinary variable name in ES 1.0.
//!
//! **A shader may overload a builtin.** `LumiGrain` defines its own
//! `float mix(float, float, float)` *and* calls the builtin
//! `mix(vec3, vec3, float)` elsewhere in the same file. Renaming every
//! occurrence of `mix` — the obvious fix for the trap above — breaks the calls
//! that resolved to the builtin. So the rename is driven by declarations, and
//! a declaration whose name is followed by `(` is a function, which *overloads*
//! rather than shadows, and is left alone.
//!
//! Both were compile errors rather than wrong pixels, which is the redeeming
//! property of this whole approach: the failures are loud.

/// Which shader stage the source is for. `varying` means opposite things in
/// the two, so this cannot be inferred from the text.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stage {
    Vertex,
    Fragment,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UniformKind {
    /// Goes in the uniform block.
    Scalar {
        glsl_type: String,
        array: Option<u32>,
    },
    /// Stays loose; glslang numbers it and step [3] splits it into a texture
    /// and a sampler.
    Sampler { glsl_type: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UniformDecl {
    pub name: String,
    pub kind: UniformKind,
}

#[derive(Debug, Clone)]
pub struct Rewritten {
    pub source: String,
    /// Every uniform the original declared, in declaration order — which is
    /// also the order they appear in the synthesised block.
    pub uniforms: Vec<UniformDecl>,
    /// How many fragment outputs were synthesised.
    pub outputs: usize,
    /// Names the rewrite had to change, `(from, to)`. Reported so a diagnostic
    /// can map a glslang error back to the author's identifier.
    pub renames: Vec<(String, String)>,
}

impl Rewritten {
    pub fn samplers(&self) -> impl Iterator<Item = &UniformDecl> {
        self.uniforms
            .iter()
            .filter(|u| matches!(u.kind, UniformKind::Sampler { .. }))
    }

    pub fn scalars(&self) -> impl Iterator<Item = &UniformDecl> {
        self.uniforms
            .iter()
            .filter(|u| matches!(u.kind, UniformKind::Scalar { .. }))
    }
}

const OPAQUE_PREFIXES: &[&str] = &["sampler", "image", "texture", "atomic_uint"];

/// Names that are legal identifiers in ES 1.0 and are keywords, or shadow a
/// builtin, in 450 core.
///
/// Deliberately only names ES 1.0 does *not* itself reserve. ES 1.0 already
/// reserves `switch`, `default`, `volatile`, `double` and the rest of that
/// list, so those can never appear as identifiers and renaming them would only
/// risk changing a program's meaning.
const RESERVED_IN_450: &[&str] = &[
    "sample",
    "patch",
    "subroutine",
    "buffer",
    "shared",
    "coherent",
    "restrict",
    "readonly",
    "writeonly",
    "atomic_uint",
    "centroid",
    "smooth",
    "noperspective",
    "layout",
    "uint",
    "uvec2",
    "uvec3",
    "uvec4",
    "resource",
    "filter",
    "common",
    "partition",
    "active",
    "row_major",
    // Not keywords. Builtins that 450 defines and ES 1.0 does not, or that the
    // rewrite itself introduces, so an ES-1.0 *variable* of this name shadows
    // them. A *function* of this name is an overload and is left alone.
    "texture",
    "textureProj",
    "textureLod",
    "textureGrad",
    "textureSize",
    "mix",
];

const ES1_TYPES: &[&str] = &[
    "void",
    "bool",
    "int",
    "float",
    "vec2",
    "vec3",
    "vec4",
    "bvec2",
    "bvec3",
    "bvec4",
    "ivec2",
    "ivec3",
    "ivec4",
    "mat2",
    "mat3",
    "mat4",
    "sampler2D",
    "samplerCube",
];

/// Blank out comments, keeping line structure, so no replacement below ever
/// edits commented-out code. The corpus is full of it.
fn strip_comments(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'/' {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                if b[i] == b'\n' {
                    out.push('\n');
                }
                i += 1;
            }
            i = (i + 2).min(b.len());
        } else {
            out.push(b[i] as char);
            i += 1;
        }
    }
    out
}

fn is_ident_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// Replace whole identifiers only — `varying` must not match inside
/// `myvaryingthing`.
fn replace_ident(src: &str, from: &str, to: &str) -> String {
    let b = src.as_bytes();
    let f = from.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < b.len() {
        if b[i..].starts_with(f)
            && (i == 0 || !is_ident_char(b[i - 1]))
            && (i + f.len() >= b.len() || !is_ident_char(b[i + f.len()]))
        {
            out.push_str(to);
            i += f.len();
        } else {
            out.push(b[i] as char);
            i += 1;
        }
    }
    out
}

fn contains_ident(src: &str, ident: &str) -> bool {
    let b = src.as_bytes();
    let f = ident.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i..].starts_with(f)
            && (i == 0 || !is_ident_char(b[i - 1]))
            && (i + f.len() >= b.len() || !is_ident_char(b[i + f.len()]))
        {
            return true;
        }
        i += 1;
    }
    false
}

fn is_opaque(ty: &str) -> bool {
    OPAQUE_PREFIXES.iter().any(|p| ty.starts_with(p))
}

/// Identifiers with their byte spans, so a declaration can be told from a call.
fn tokens_at(src: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let b = src.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if is_ident_char(b[i]) && !b[i].is_ascii_digit() {
            let start = i;
            while i < b.len() && is_ident_char(b[i]) {
                i += 1;
            }
            out.push((i, &src[start..i]));
        } else {
            i += 1;
        }
    }
    out
}

fn next_nonspace(src: &str, from: usize) -> Option<u8> {
    src.as_bytes()[from..]
        .iter()
        .copied()
        .find(|c| !c.is_ascii_whitespace())
}

fn rename_reserved(src: &str) -> (String, Vec<(String, String)>) {
    let toks = tokens_at(src);
    let mut renames: Vec<(String, String)> = Vec::new();
    for pair in toks.windows(2) {
        let (_, ty) = pair[0];
        let (end, name) = pair[1];
        if !ES1_TYPES.contains(&ty) || !RESERVED_IN_450.contains(&name) {
            continue;
        }
        if next_nonspace(src, end) == Some(b'(') {
            continue; // a function: overloads rather than shadows
        }
        if !renames.iter().any(|(f, _)| f == name) {
            renames.push((name.to_string(), format!("{name}_")));
        }
    }
    let mut out = src.to_string();
    for (from, to) in &renames {
        out = replace_ident(&out, from, to);
    }
    (out, renames)
}

/// `uniform [precision] <type> <name>[[N]];`
///
/// One declarator per statement, which is what the corpus does everywhere —
/// it is machine output from ByteDance's `.ausl` compiler and hand-written
/// shaders in the same style. A comma-separated declarator list would be
/// missed, and would then fail loudly in glslang rather than silently here.
fn parse_uniform(line: &str) -> Option<UniformDecl> {
    let rest = line.strip_prefix("uniform ")?;
    let rest = rest.strip_suffix(';')?.trim();
    if rest.contains(',') || rest.contains('{') {
        return None;
    }
    let mut words: Vec<&str> = rest.split_whitespace().collect();
    while !words.is_empty() && matches!(words[0], "highp" | "mediump" | "lowp") {
        words.remove(0);
    }
    if words.len() < 2 {
        return None;
    }
    let glsl_type = words[0].to_string();
    let mut name = words[1..].concat();
    let mut array = None;
    if let Some(open) = name.find('[') {
        let close = name.find(']')?;
        array = name[open + 1..close].trim().parse::<u32>().ok();
        name = name[..open].to_string();
    }
    if name.is_empty() {
        return None;
    }
    let kind = if is_opaque(&glsl_type) {
        UniformKind::Sampler { glsl_type }
    } else {
        UniformKind::Scalar { glsl_type, array }
    };
    Some(UniformDecl { name, kind })
}

fn uniform_block(uniforms: &[UniformDecl]) -> String {
    let scalars: Vec<&UniformDecl> = uniforms
        .iter()
        .filter(|u| matches!(u.kind, UniformKind::Scalar { .. }))
        .collect();
    if scalars.is_empty() {
        return String::new();
    }
    let mut block = String::from("layout(std140) uniform ChukcutUniforms {\n");
    for u in scalars {
        if let UniformKind::Scalar { glsl_type, array } = &u.kind {
            match array {
                Some(n) => block.push_str(&format!("    {glsl_type} {}[{n}];\n", u.name)),
                None => block.push_str(&format!("    {glsl_type} {};\n", u.name)),
            }
        }
    }
    block.push_str("};\n");
    block
}

/// Rewrite ES 1.0 (or ES 3.0) source into GLSL 450 core.
pub fn rewrite(source: &str, stage: Stage) -> Rewritten {
    let clean = strip_comments(source);
    let (clean, renames) = rename_reserved(&clean);

    // ES 3.0 already writes `in`/`out` and `texture`, so only the version line,
    // the precision statements and the uniform block need touching. Four of the
    // 228 sampled fragment shaders are in this dialect.
    let es3 = clean.contains("#version 300") || clean.contains("#version 310");

    let mut uniforms = Vec::new();
    let mut prelude = String::new();
    let mut body = String::new();

    for raw in clean.lines() {
        let line = raw.trim();
        if line.starts_with("#version") {
            continue;
        }
        // Legal in 450 core, but they mean nothing there and glslang only emits
        // `RelaxedPrecision` decorations that naga logs as unknown and ignores.
        if line.starts_with("precision ") && line.ends_with(';') {
            continue;
        }
        if let Some(decl) = parse_uniform(line) {
            if let UniformKind::Sampler { glsl_type } = &decl.kind {
                prelude.push_str(&format!("uniform {glsl_type} {};\n", decl.name));
            }
            uniforms.push(decl);
            continue;
        }
        body.push_str(raw);
        body.push('\n');
    }

    if !es3 {
        body = match stage {
            Stage::Vertex => replace_ident(&body, "varying", "out"),
            Stage::Fragment => replace_ident(&body, "varying", "in"),
        };
        body = replace_ident(&body, "attribute", "in");
        for (from, to) in [
            ("texture2DProjLod", "textureProjLod"),
            ("texture2DProj", "textureProj"),
            ("texture2DLod", "textureLod"),
            ("texture2D", "texture"),
            ("textureCubeLod", "textureLod"),
            ("textureCube", "texture"),
        ] {
            body = replace_ident(&body, from, to);
        }
    }

    for p in ["highp ", "mediump ", "lowp "] {
        body = body.replace(p, "");
    }

    let mut outputs = 0usize;
    if stage == Stage::Fragment && !es3 {
        let mut decls = String::new();
        if contains_ident(&body, "gl_FragColor") {
            decls.push_str("layout(location = 0) out vec4 chukcut_FragColor;\n");
            body = replace_ident(&body, "gl_FragColor", "chukcut_FragColor");
            outputs = 1;
        }
        // `gl_FragData[N]` — 72 of the sampled shaders write it. Only the
        // indices actually written get an output, because declaring unused
        // ones would demand colour attachments the pass does not have.
        let highest = (0..8)
            .filter(|n| body.contains(&format!("gl_FragData[{n}]")))
            .max();
        if let Some(hi) = highest {
            for n in 0..=hi {
                decls.push_str(&format!(
                    "layout(location = {}) out vec4 chukcut_FragData{n};\n",
                    n + outputs
                ));
                body = body.replace(
                    &format!("gl_FragData[{n}]"),
                    &format!("chukcut_FragData{n}"),
                );
            }
            outputs += hi + 1;
        }
        prelude.push_str(&decls);
    } else if stage == Stage::Fragment {
        outputs = 1;
    }

    let mut out = String::from("#version 450 core\n");
    out.push_str(&prelude);
    out.push_str(&uniform_block(&uniforms));
    out.push_str(&body);

    Rewritten {
        source: out,
        uniforms,
        outputs,
        renames,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ES1: &str = r#"
// a comment mentioning varying and texture2D, which must survive untouched
precision highp float;
varying vec2 v_uv;
uniform sampler2D u_inputTexture;
uniform float u_intensity;
uniform vec2 u_center;
uniform mediump sampler2D u_maskTexture;

void main() {
    vec4 c = texture2D(u_inputTexture, v_uv);
    gl_FragColor = c * u_intensity;
}
"#;

    #[test]
    fn rewrites_the_es1_dialect_into_450_core() {
        let out = rewrite(ES1, Stage::Fragment);
        assert!(out.source.starts_with("#version 450 core\n"));
        assert!(!out.source.contains("precision highp float;"));
        assert!(!contains_ident(&out.source, "varying"));
        assert!(!contains_ident(&out.source, "texture2D"));
        assert!(out.source.contains("in vec2 v_uv;"));
        assert!(out
            .source
            .contains("layout(location = 0) out vec4 chukcut_FragColor;"));
    }

    #[test]
    fn loose_scalar_uniforms_move_into_a_block_and_samplers_do_not() {
        // Vulkan GLSL rejects a loose non-opaque uniform, and a sampler cannot
        // go in a block. Getting this backwards fails at glslang, loudly.
        let out = rewrite(ES1, Stage::Fragment);
        assert!(out
            .source
            .contains("layout(std140) uniform ChukcutUniforms {"));
        assert!(out.source.contains("    float u_intensity;"));
        assert!(out.source.contains("    vec2 u_center;"));
        assert!(out.source.contains("uniform sampler2D u_inputTexture;"));

        let samplers: Vec<&str> = out.samplers().map(|u| u.name.as_str()).collect();
        assert_eq!(samplers, ["u_inputTexture", "u_maskTexture"]);
        let scalars: Vec<&str> = out.scalars().map(|u| u.name.as_str()).collect();
        assert_eq!(scalars, ["u_intensity", "u_center"]);
    }

    #[test]
    fn varying_becomes_an_output_in_a_vertex_shader_and_an_input_in_a_fragment_one() {
        let source = "attribute vec4 a_position;\nvarying vec2 v_uv;\nvoid main() {}\n";
        let vertex = rewrite(source, Stage::Vertex);
        assert!(vertex.source.contains("out vec2 v_uv;"));
        assert!(vertex.source.contains("in vec4 a_position;"));

        let fragment = rewrite(source, Stage::Fragment);
        assert!(fragment.source.contains("in vec2 v_uv;"));
    }

    #[test]
    fn a_variable_named_sample_is_renamed_because_450_reads_it_as_a_qualifier() {
        // Found by running the corpus: a Sobel filter declares `vec3 sample;`.
        let source =
            "precision highp float;\nvoid main() {\n  vec3 sample;\n  sample = vec3(1.0);\n}\n";
        let out = rewrite(source, Stage::Fragment);
        assert!(!contains_ident(&out.source, "sample"));
        assert!(out.source.contains("vec3 sample_;"));
        assert_eq!(out.renames, [("sample".to_string(), "sample_".to_string())]);
    }

    #[test]
    fn a_shader_that_overloads_a_builtin_keeps_calling_the_builtin() {
        // Also found by running the corpus: LumiGrain defines its own
        // `float mix(float, float, float)` and calls the builtin
        // `mix(vec3, vec3, float)` in the same file. Renaming the definition
        // breaks every call that resolved to the builtin, so a declaration
        // followed by `(` is left alone.
        let source = concat!(
            "precision highp float;\n",
            "float mix(float a, float b, float t) { return a; }\n",
            "void main() { vec3 c = mix(vec3(0.0), vec3(1.0), 0.5); }\n",
        );
        let out = rewrite(source, Stage::Fragment);
        assert!(out.renames.is_empty(), "renamed: {:?}", out.renames);
        assert!(out.source.contains("float mix(float a, float b, float t)"));
    }

    #[test]
    fn commented_out_code_is_never_rewritten() {
        let out = rewrite(ES1, Stage::Fragment);
        // The comment mentioned both, and neither was touched — the comment is
        // gone entirely, which is the only safe thing to do with it.
        assert!(!out.source.contains("a comment mentioning"));
    }

    #[test]
    fn gl_fragdata_becomes_one_declared_output_per_index_written() {
        let source = concat!(
            "precision highp float;\n",
            "void main() {\n",
            "  gl_FragData[0] = vec4(1.0);\n",
            "  gl_FragData[1] = vec4(0.0);\n",
            "}\n",
        );
        let out = rewrite(source, Stage::Fragment);
        assert_eq!(out.outputs, 2);
        assert!(out
            .source
            .contains("layout(location = 0) out vec4 chukcut_FragData0;"));
        assert!(out
            .source
            .contains("layout(location = 1) out vec4 chukcut_FragData1;"));
        assert!(!out.source.contains("gl_FragData"));
    }

    #[test]
    fn an_array_uniform_keeps_its_length() {
        let source = "uniform float u_weights[9];\nvoid main() {}\n";
        let out = rewrite(source, Stage::Fragment);
        assert!(out.source.contains("    float u_weights[9];"));
        assert_eq!(
            out.uniforms[0].kind,
            UniformKind::Scalar {
                glsl_type: "float".into(),
                array: Some(9)
            }
        );
    }
}
