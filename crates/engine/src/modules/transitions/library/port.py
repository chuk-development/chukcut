#!/usr/bin/env python3
"""Port the gl-transitions catalogue to WGSL.

gl-transitions (https://github.com/gl-transitions/gl-transitions) is a set of
GLSL transition shaders, almost all under the MIT licence. This script turns
each one into a WGSL module that chukcut's transition renderer can load, and
writes the table the renderer and the catalog read.

It is a development tool. Its output is committed; nothing runs it at build
time, so a build never needs the network or the naga CLI.

    python3 port.py gl-transitions.json path/to/naga

The JSON is the npm package's `gl-transitions.json`
(https://cdn.jsdelivr.net/npm/gl-transitions/gl-transitions.json). `naga` is
naga-cli 30, the version wgpu 30 links, so the WGSL it writes is WGSL the
runtime accepts.

How one transition becomes WGSL:

1. Its `uniform` lines are removed, and each parameter name is replaced by a
   slot of one uniform array: `params[i].x` for a float, `.xy` for a vec2,
   `int(...)` around an int, `(... > 0.5)` around a bool. The renderer fills
   the array from the transition's stored values, so a parameter is data, not
   a recompiled shader.
2. `progress` comes from the same uniform block; `ratio` is measured from
   the layer textures.
3. The body is wrapped in a GLSL 450 harness that supplies `getFromColor` and
   `getToColor` and an entry point, and naga translates the whole to WGSL.

Two things the harness does on purpose:

- **Premultiplied mixing.** chukcut's layers are straight alpha and carry
  transparent letterbox bars. The samplers hand the transition premultiplied
  colour and the entry point unpremultiplies the result, so a `mix()` written
  for opaque images does not drag a visible pixel towards the colour of a
  transparent one.
  (The pipeline blends premultiplied colour, so `mod.rs` draws each port
  through a second entry, `chukcut_main`, that calls the translated `main_1`
  and multiplies by alpha again. Keep the names `main_1`, `v_uv_1` and
  `o_color` stable, or change `GL_OUTPUT_WGSL` with them.)
- **GL texture coordinates.** gl-transitions puts the origin at the bottom
  left, chukcut at the top left. The harness flips `y` on the way in and on
  every sample, so "wipe up" still wipes up.

Transitions that are left out, and why, are listed in `SKIP` below.
"""

import json
import os
import re
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
OUT_DIR = os.path.join(HERE, "gl")
TABLE = os.path.join(HERE, "gl_table.rs")

# Parameter slots in the uniform block. Revolve_Left has the most, ten.
SLOTS = 12

SKIP = {
    # Need an extra image input (a displacement map, a luma matte), which the
    # renderer does not provide.
    "displacement": "needs a displacement map texture",
    "luma": "needs a luma matte texture",
    # Their noise function cites a Shadertoy page as its origin. Shadertoy's
    # default licence is CC BY-NC-SA, which is not compatible with the GPL;
    # docs/research/open-assets.md says to leave such code out.
    "burn0": "noise function credited to a Shadertoy page",
    "perlin": "noise function credited to a Shadertoy page",
    # Ported from a blog post with no stated licence.
    "tangentMotionBlur": "ported from a source with no stated licence",
}

HARNESS_HEAD = """#version 450
layout(location = 0) in vec2 v_uv;
layout(location = 0) out vec4 o_color;
layout(set = 0, binding = 0) uniform GlBlock {
    vec4 state;
    vec4 params[%d];
} U;
layout(set = 1, binding = 0) uniform texture2D u_from;
layout(set = 1, binding = 1) uniform texture2D u_to;
layout(set = 1, binding = 2) uniform sampler u_sampler;

vec4 chukcut_premultiply(vec4 c) { return vec4(c.rgb * c.a, c.a); }
vec4 getFromColor(vec2 uv) {
    return chukcut_premultiply(texture(sampler2D(u_from, u_sampler), vec2(uv.x, 1.0 - uv.y)));
}
vec4 getToColor(vec2 uv) {
    return chukcut_premultiply(texture(sampler2D(u_to, u_sampler), vec2(uv.x, 1.0 - uv.y)));
}
""" % SLOTS

HARNESS_TAIL = """
void main() {
    progress = U.state.x;
    // The aspect of the layers themselves, so the renderer never has to
    // pass the canvas size down to a transition.
    ivec2 dims = textureSize(sampler2D(u_from, u_sampler), 0);
    ratio = float(dims.x) / float(max(dims.y, 1));
%s
    chukcut_init_globals();
    vec4 c = transition(vec2(v_uv.x, 1.0 - v_uv.y));
    // A few transitions add to alpha as if it were a colour channel; GL
    // clamps it on write, so clamp it here before unpremultiplying.
    float a = clamp(c.a, 0.0, 1.0);
    o_color = a > 0.00001 ? vec4(c.rgb / a, a) : vec4(0.0);
}
"""

ACCESS = {
    "float": "U.params[{i}].x",
    "int": "int(U.params[{i}].x)",
    "bool": "(U.params[{i}].x > 0.5)",
    "vec2": "U.params[{i}].xy",
    "vec3": "U.params[{i}].xyz",
    "vec4": "U.params[{i}]",
    "ivec2": "ivec2(U.params[{i}].xy)",
}

KINDS = {
    "float": "Float",
    "int": "Int",
    "bool": "Bool",
    "vec2": "Vec2",
    "vec3": "Color3",
    "vec4": "Color4",
    "ivec2": "IVec2",
}

TYPES = r"(?:float|int|bool|vec2|vec3|vec4|ivec2|mat2|mat3)"


def label(name):
    """`CrossZoom` -> `Cross zoom`, `wipeLeft` -> `Wipe left`."""
    words = re.sub(r"[_\-]+", " ", name)
    words = re.sub(r"(?<=[a-z0-9])(?=[A-Z])", " ", words)
    words = " ".join(words.split()).lower()
    words = words.replace("rigth", "right")
    return words[:1].upper() + words[1:]


def default_values(kind, value):
    """Four floats for a parameter's default, whatever its type."""
    if isinstance(value, bool):
        return [1.0 if value else 0.0, 0.0, 0.0, 0.0]
    if isinstance(value, (int, float)):
        return [float(value), 0.0, 0.0, 0.0]
    values = [float(v) for v in value]
    if kind == "ivec2" and len(values) == 1:
        values = values * 2
    return (values + [0.0, 0.0, 0.0, 0.0])[:4]


def parse_default(kind, text):
    """The numbers in a `// = vec3(0.9, 0.4, 0.2)` style default."""
    text = text.strip().lower()
    if text in ("true", "false"):
        return [1.0 if text == "true" else 0.0, 0.0, 0.0, 0.0]
    numbers = [float(n) for n in re.findall(r"-?\d*\.?\d+(?:e-?\d+)?", text)]
    if kind in ("vec2", "ivec2", "vec3", "vec4") and len(numbers) == 1:
        numbers = numbers * {"vec2": 2, "ivec2": 2, "vec3": 3, "vec4": 4}[kind]
    return (numbers + [0.0, 0.0, 0.0, 0.0])[:4]


def parameters(entry):
    """Every `uniform` the transition declares, with type and default.

    Read from the source rather than from `paramsTypes`, because a handful of
    transitions declare a uniform whose default sits in a block comment and
    the npm table then lists no parameters at all.
    """
    params = []
    for line in entry["glsl"].splitlines():
        m = re.match(r"\s*uniform\s+(\w+)\s+(\w+)\s*(?:/\*\s*=\s*(.*?)\*/)?\s*;\s*(?://\s*=\s*(.*))?$", line)
        if not m:
            continue
        kind, name, block_default, line_default = m.groups()
        if kind not in ACCESS:
            raise ValueError(f"unsupported uniform type {kind}")
        if name in entry["defaultParams"]:
            default = default_values(kind, entry["defaultParams"][name])
        else:
            default = parse_default(kind, block_default or line_default or "0")
        params.append((name, kind, default))
    return params


def rewrite(entry, params):
    """The transition's GLSL, its uniforms turned into private globals.

    A parameter becomes a global variable assigned from its slot at the top of
    `main`, rather than a textual substitution of every use, so a function
    argument or a local that happens to share a parameter's name still means
    what it meant. Globals initialised from `progress` (GLSL ES evaluates
    those per invocation) are moved into `chukcut_init_globals`, which `main`
    calls once the parameters are set.
    """
    if len(params) > SLOTS:
        raise ValueError(f"{len(params)} parameters, the block has {SLOTS}")
    lines = []
    inits = []
    depth = 0
    for line in entry["glsl"].splitlines():
        stripped = line.strip()
        if re.match(r"uniform\s+\w+\s+\w+", stripped):
            continue
        if stripped.startswith("precision "):
            continue
        if depth == 0:
            m = re.match(r"(%s)\s+(\w+)\s*=\s*(.+);\s*(//.*)?$" % TYPES, stripped)
            if m and not stripped.startswith("const"):
                kind, name, expr, _ = m.groups()
                lines.append("%s %s;" % (kind, name))
                inits.append("    %s = %s;" % (name, expr))
                continue
        depth += line.count("{") - line.count("}")
        lines.append(line)
    body = "\n".join(lines)
    body = re.sub(r"#ifdef\s+GL_ES\s*#endif", "", body)
    # GLSL ES lets a shader overload `texture`; GLSL 450 has it as a builtin
    # that naga will not let a user function shadow.
    body = re.sub(r"\btexture\s*\(", "chukcut_texture(", body)
    globals_ = "float progress;\nfloat ratio;\n" + "".join(
        "%s %s;\n" % (kind, name) for name, kind, _ in params
    )
    init = "void chukcut_init_globals() {\n" + "\n".join(inits) + "\n}\n"
    assigns = "\n".join(
        "    %s = %s;" % (name, ACCESS[kind].format(i=i)) for i, (name, kind, _) in enumerate(params)
    )
    return globals_ + body + "\n" + init, assigns


def header_comments(glsl):
    """Every comment before the first line of code, as `//` lines.

    That is where the author, the licence and any copyright notice live; a BSD
    licence requires the notice to be kept with the source, and two of the
    transitions carry theirs in a block comment, one of them after its
    uniforms.
    """
    out = []
    in_block = False
    for line in glsl.splitlines():
        s = line.strip()
        if in_block:
            end = s.find("*/")
            text = s if end < 0 else s[:end]
            out.append(("// " + text).rstrip())
            if end >= 0:
                in_block = False
            continue
        if not s:
            continue
        if s.startswith("//"):
            out.append(s)
            continue
        if s.startswith("/*"):
            text = s[2:]
            end = text.find("*/")
            if end >= 0:
                out.append(("// " + text[:end].strip()).rstrip())
            else:
                out.append(("// " + text.strip()).rstrip())
                in_block = True
            continue
        if s.startswith("uniform ") or s.startswith("precision ") or s.startswith("#"):
            continue
        break
    while out and out[-1] == "//":
        out.pop()
    return out


def port(entry, naga):
    body, assigns = rewrite(entry, parameters(entry))
    source = HARNESS_HEAD + body + HARNESS_TAIL % assigns
    with tempfile.TemporaryDirectory(dir=os.path.join(HERE, "..", "..", "..", "..", "..", "..", "_scratch")) as tmp:
        frag = os.path.join(tmp, "t.frag")
        wgsl = os.path.join(tmp, "t.wgsl")
        with open(frag, "w") as f:
            f.write(source)
        result = subprocess.run([naga, "--input-kind", "glsl", "--shader-stage", "frag", frag, wgsl], capture_output=True, text=True)
        if result.returncode != 0:
            raise RuntimeError(result.stderr.strip() or result.stdout.strip())
        with open(wgsl) as f:
            return f.read()


def main():
    catalogue_path, naga = sys.argv[1], sys.argv[2]
    with open(catalogue_path) as f:
        catalogue = json.load(f)
    os.makedirs(OUT_DIR, exist_ok=True)
    rows = []
    failed = []
    for entry in sorted(catalogue, key=lambda e: e["name"].lower()):
        name = entry["name"]
        if name in SKIP:
            continue
        try:
            wgsl = port(entry, naga)
        except Exception as error:  # noqa: BLE001 - reported, not hidden
            failed.append((name, str(error).splitlines()[0:3]))
            continue
        ident = re.sub(r"[^a-z0-9]+", "_", name.lower()).strip("_")
        header = header_comments(entry["glsl"])
        lines = [
            "// %s, from gl-transitions (https://github.com/gl-transitions/gl-transitions)." % name,
            "// Author: %s" % entry["author"],
            "// License: %s" % entry["license"],
        ]
        lines += [h for h in header if not re.match(r"//\s*(Author|License)\s*:", h, re.I)]
        lines += [
            "//",
            "// Translated from GLSL to WGSL by naga 30 through the harness in",
            "// `../port.py`; the licence texts are in `../LICENSE-gl-transitions.md`.",
            "// Edit `port.py`, not this file.",
            "",
        ]
        with open(os.path.join(OUT_DIR, ident + ".wgsl"), "w") as f:
            f.write("\n".join(lines) + wgsl)
        params = parameters(entry)
        rows.append((ident, name, label(name), entry["author"], entry["license"], params))

    with open(TABLE, "w") as f:
        f.write("// Generated by `port.py` from gl-transitions. Do not edit by hand.\n")
        f.write("// One row per ported transition: id, upstream name, label, author,\n")
        f.write("// licence, WGSL source and parameters.\n\n")
        f.write("use super::{GlEntry, GlParam, GlParamKind};\n\n")
        f.write("pub(super) const GL_TRANSITIONS: &[GlEntry] = &[\n")
        for ident, name, lab, author, licence, params in rows:
            f.write("    GlEntry {\n")
            f.write("        id: %s,\n" % json.dumps(ident))
            f.write("        upstream: %s,\n" % json.dumps(name))
            f.write("        label: %s,\n" % json.dumps(lab))
            f.write("        author: %s,\n" % json.dumps(author, ensure_ascii=False))
            f.write("        license: %s,\n" % json.dumps(licence))
            f.write("        wgsl: include_str!(\"gl/%s.wgsl\"),\n" % ident)
            f.write("        params: &[\n")
            for pname, kind, default in params:
                f.write(
                    "            GlParam { name: %s, kind: GlParamKind::%s, default: [%s] },\n"
                    % (json.dumps(pname), KINDS[kind], ", ".join(repr(float(v)) for v in default))
                )
            f.write("        ],\n")
            f.write("    },\n")
        f.write("];\n")

    print(f"ported {len(rows)}, skipped {len(SKIP)}, failed {len(failed)}")
    for name, error in failed:
        print(f"  {name}: {error}")


if __name__ == "__main__":
    main()
