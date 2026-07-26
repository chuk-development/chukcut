// Transitions: one fragment shader per kind, over the same two layers.
//
// Each entry point receives two full-canvas textures — the outgoing
// composition and the incoming one, already transformed, cropped and faded by
// the compositor's own quad pass — plus a progress value in `0..1` that has
// already been through the transition's easing. Nothing in here knows about
// time, clips, handles or microseconds; that arithmetic lives in
// `transitions/resolve.rs`, where it is testable without a GPU.
//
// ## Straight alpha, and why the mixing looks fussy
//
// The compositor works in straight (non-premultiplied) alpha, and a straight
// `mix()` of two straight-alpha colours is wrong wherever their alphas differ:
// a fully transparent pixel still carries a colour, and lerping towards it
// drags the visible pixel towards that colour instead of towards transparency.
// The letterbox bars around a 16:9 clip on a 9:16 canvas are exactly that case,
// so the bug shows up as a dark halo creeping in from the edges. Every blend
// here therefore premultiplies, mixes, and unpremultiplies.
//
// ## Colour space
//
// The layer textures are `Rgba8UnormSrgb`, so sampling decodes to linear light
// and writing to an sRGB target encodes back. All the arithmetic below is in
// linear light, which is where a crossfade belongs — an sRGB-space crossfade
// darkens through the middle.

struct TransitionUniform {
    // Eased position through the transition, 0 at the start of the window and
    // approaching 1 at its end. Never exactly 1: the window is half-open.
    progress: f32,
    // 0 = left, 1 = right, 2 = up, 3 = down. Matches
    // `TransitionDirection::shader_index`.
    direction: u32,
    // Wipe: width of the softened edge as a fraction of the frame.
    softness: f32,
    // Zoom: extra scale the push adds.
    zoom: f32,
    // Dip: the colour dipped to, straight-alpha linear RGBA.
    color: vec4<f32>,
};

@group(0) @binding(0) var<uniform> tr: TransitionUniform;

@group(1) @binding(0) var from_texture: texture_2d<f32>;
@group(1) @binding(1) var to_texture: texture_2d<f32>;
@group(1) @binding(2) var layer_sampler: sampler;

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

// A fullscreen triangle, not a quad: three vertices with no vertex buffer at
// all, and no diagonal seam down the middle where two triangles meet and the
// rasterizer runs quads of fragments twice.
//
// Clip space is y-up and UV space is y-down, so the vertex at `y = +1` is
// `v = 0`. Getting that backwards renders every transition upside down.
@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VertexOutput {
    let x = f32((index << 1u) & 2u);
    let y = f32(index & 2u);
    var out: VertexOutput;
    out.clip_position = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    out.uv = vec2<f32>(x, y);
    return out;
}

fn premultiply(c: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(c.rgb * c.a, c.a);
}

fn unpremultiply(c: vec4<f32>) -> vec4<f32> {
    if (c.a <= 0.00001) {
        return vec4<f32>(0.0, 0.0, 0.0, 0.0);
    }
    return vec4<f32>(c.rgb / c.a, c.a);
}

// Cross-fade two straight-alpha colours. `k` of 0 is all `a`, 1 is all `b`.
fn mix_layers(a: vec4<f32>, b: vec4<f32>, k: f32) -> vec4<f32> {
    return unpremultiply(mix(premultiply(a), premultiply(b), k));
}

// Source-over: `top` composited onto `bottom`, both straight alpha.
fn over(top: vec4<f32>, bottom: vec4<f32>) -> vec4<f32> {
    let t = premultiply(top);
    let b = premultiply(bottom);
    return unpremultiply(t + b * (1.0 - t.a));
}

fn sample_from(uv: vec2<f32>) -> vec4<f32> {
    return textureSample(from_texture, layer_sampler, uv);
}

fn sample_to(uv: vec2<f32>) -> vec4<f32> {
    return textureSample(to_texture, layer_sampler, uv);
}

// Whether a sample coordinate is still on the canvas. The sampler clamps to
// the edge, so without this a translated layer smears its border pixels across
// everything it has already left.
fn on_canvas(uv: vec2<f32>) -> bool {
    return uv.x >= 0.0 && uv.x < 1.0 && uv.y >= 0.0 && uv.y < 1.0;
}

// The unit vector, in UV space, a directional transition travels along.
fn travel() -> vec2<f32> {
    switch tr.direction {
        case 0u: { return vec2<f32>(-1.0, 0.0); }
        case 1u: { return vec2<f32>(1.0, 0.0); }
        case 2u: { return vec2<f32>(0.0, -1.0); }
        default: { return vec2<f32>(0.0, 1.0); }
    }
}

// ---------------------------------------------------------------------------
// Cross-dissolve
// ---------------------------------------------------------------------------

@fragment
fn fs_dissolve(in: VertexOutput) -> @location(0) vec4<f32> {
    return mix_layers(sample_from(in.uv), sample_to(in.uv), tr.progress);
}

// ---------------------------------------------------------------------------
// Dip to colour
// ---------------------------------------------------------------------------

@fragment
fn fs_dip(in: VertexOutput) -> @location(0) vec4<f32> {
    // Two halves, not a crossfade: the outgoing clip fades to the colour, then
    // the colour fades to the incoming one. A crossfade with a colour laid over
    // it would show both clips through the dip, which is the one thing a dip is
    // supposed to prevent.
    //
    // The veil covers the whole canvas rather than just the clip, because "dip
    // to black" means the frame goes black — a letterboxed clip dipping to
    // black inside its own rectangle only looks like a bug.
    let dip = 1.0 - abs(2.0 * tr.progress - 1.0);
    let base = select(sample_from(in.uv), sample_to(in.uv), tr.progress >= 0.5);
    let veil = vec4<f32>(tr.color.rgb, tr.color.a * dip);
    return over(veil, base);
}

// ---------------------------------------------------------------------------
// Wipe
// ---------------------------------------------------------------------------

@fragment
fn fs_wipe(in: VertexOutput) -> @location(0) vec4<f32> {
    let d = travel();
    // Distance along the travel axis, 0 at the edge the wipe starts from and 1
    // at the edge it ends on. One expression for all four directions.
    let along = dot(in.uv - vec2<f32>(0.5, 0.5), d) + 0.5;

    let soft = max(tr.softness, 0.0005);
    // The edge has to start one softness-width off the canvas and finish on the
    // far edge, or a soft wipe would show a band of the incoming clip at
    // progress 0 and never quite finish at progress 1.
    let edge = tr.progress * (1.0 + soft) - soft;
    let k = 1.0 - smoothstep(edge, edge + soft, along);
    return mix_layers(sample_from(in.uv), sample_to(in.uv), k);
}

// ---------------------------------------------------------------------------
// Slide (push)
// ---------------------------------------------------------------------------

@fragment
fn fs_slide(in: VertexOutput) -> @location(0) vec4<f32> {
    // Push, not slide-over: both layers travel together, so the incoming clip
    // appears to shove the outgoing one out of frame rather than to glide over
    // a stationary picture.
    let d = travel();
    let from_uv = in.uv - d * tr.progress;
    let to_uv = in.uv + d * (1.0 - tr.progress);

    let from_visible = on_canvas(from_uv);
    let to_visible = on_canvas(to_uv);

    let f = select(vec4<f32>(0.0, 0.0, 0.0, 0.0), sample_from(from_uv), from_visible);
    let t = select(vec4<f32>(0.0, 0.0, 0.0, 0.0), sample_to(to_uv), to_visible);
    // They never overlap — one layer occupies exactly the frame the other has
    // vacated — so this is a selection, not a blend.
    return select(f, t, to_visible);
}

// ---------------------------------------------------------------------------
// Zoom
// ---------------------------------------------------------------------------

@fragment
fn fs_zoom(in: VertexOutput) -> @location(0) vec4<f32> {
    // The outgoing clip pushes towards the viewer while the incoming one
    // settles back from the same place, crossfading through the middle, so the
    // cut reads as one continuous move rather than two.
    let amount = max(tr.zoom, 0.0);
    let centre = vec2<f32>(0.5, 0.5);
    let from_scale = 1.0 + amount * tr.progress;
    let to_scale = 1.0 + amount * (1.0 - tr.progress);
    let from_uv = centre + (in.uv - centre) / from_scale;
    let to_uv = centre + (in.uv - centre) / to_scale;
    return mix_layers(sample_from(from_uv), sample_to(to_uv), tr.progress);
}
