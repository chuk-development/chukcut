// Built-in effects: one fullscreen fragment entry point per pass.
//
// The effect renderer (`fx/render.rs`) runs a clip's effect stack over a
// full-canvas layer, pass by pass, between pooled textures. Everything here is
// written from the textbook descriptions of each technique; nothing is copied
// from Shadertoy or LYGIA (see docs/research/open-assets.md).
//
// ## Conventions every entry point follows
//
// - **Premultiplied, linear light.** `fs_import` reads the layer (straight
//   alpha; sampling the sRGB texture decodes to linear) and premultiplies.
//   Every effect pass reads and writes premultiplied linear colour in
//   `Rgba16Float`, which is what makes a blur or a glow over a transparent
//   letterbox bar correct: there is no colour hiding under zero alpha to
//   bleed in. `fs_export` unpremultiplies for the compositor.
// - **Sizes in output pixels.** The CPU converts every length the user set
//   (as a fraction of the frame) into pixels of the frame being drawn, so a
//   small preview and a large export look the same. `fx.frame.xy` is the
//   full frame size, `fx.out_size` the size of the pass target (smaller for a
//   downsampled pass), `fx.in_size` the size of input 0.
// - **Outside the picture is transparent.** The sampler clamps to the edge,
//   so a pass that moves the picture checks `on_frame` and returns nothing
//   rather than smearing the border pixels.

struct FxUniform {
    // Pass target: width, height, 1/width, 1/height.
    out_size: vec4<f32>,
    // Input 0: width, height, 1/width, 1/height.
    in_size: vec4<f32>,
    // Full frame width and height in pixels, clip time in seconds, seed.
    frame: vec4<f32>,
    // Effect parameters; meaning per entry point.
    p: array<vec4<f32>, 6>,
};

@group(0) @binding(0) var<uniform> fx: FxUniform;
@group(1) @binding(0) var t0: texture_2d<f32>;
@group(1) @binding(1) var t1: texture_2d<f32>;
@group(1) @binding(2) var s0: sampler;

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

// A fullscreen triangle. UV origin top left, y down.
@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VertexOutput {
    let x = f32((index << 1u) & 2u);
    let y = f32(index & 2u);
    var out: VertexOutput;
    out.clip_position = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    out.uv = vec2<f32>(x, y);
    return out;
}

const LUMA: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);
const TAU: f32 = 6.2831853;

fn on_frame(uv: vec2<f32>) -> bool {
    return uv.x >= 0.0 && uv.x <= 1.0 && uv.y >= 0.0 && uv.y <= 1.0;
}

fn sample0(uv: vec2<f32>) -> vec4<f32> {
    return textureSampleLevel(t0, s0, uv, 0.0);
}

fn sample1(uv: vec2<f32>) -> vec4<f32> {
    return textureSampleLevel(t1, s0, uv, 0.0);
}

// Input 0 at an exact texel, clamped to the texture.
fn load0(p: vec2<i32>) -> vec4<f32> {
    let dims = vec2<i32>(textureDimensions(t0));
    return textureLoad(t0, clamp(p, vec2<i32>(0), dims - vec2<i32>(1)), 0);
}

// A sample that is transparent off the frame.
fn sample_on(uv: vec2<f32>) -> vec4<f32> {
    if (!on_frame(uv)) {
        return vec4<f32>(0.0);
    }
    return sample0(uv);
}

// PCG, the integer hash the grade's grain uses (render/shaders/quad.wgsl):
// identical on every GPU and every run, so the preview and the export agree.
fn pcg(v: u32) -> u32 {
    let state = v * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}

// A hash in 0..1 of two integers and the seed.
fn hash2(x: u32, y: u32) -> f32 {
    return f32(pcg(x + pcg(y + pcg(u32(fx.frame.w)))) & 65535u) / 65535.0;
}

fn seed_u() -> u32 {
    return u32(fx.frame.w);
}

// ---------------------------------------------------------------------------
// The ends of a chain
// ---------------------------------------------------------------------------

// Straight-alpha layer in, premultiplied out.
@fragment
fn fs_import(in: VertexOutput) -> @location(0) vec4<f32> {
    let c = load0(vec2<i32>(in.clip_position.xy));
    return vec4<f32>(c.rgb * c.a, c.a);
}

// Premultiplied in, straight alpha out.
@fragment
fn fs_export(in: VertexOutput) -> @location(0) vec4<f32> {
    let c = load0(vec2<i32>(in.clip_position.xy));
    if (c.a <= 0.00001) {
        return vec4<f32>(0.0);
    }
    return vec4<f32>(c.rgb / c.a, min(c.a, 1.0));
}

// A straight-alpha texture drawn as it is: composited by the pipeline's blend
// state (source-over for an effected clip, none for an adjustment's result).
@fragment
fn fs_copy(in: VertexOutput) -> @location(0) vec4<f32> {
    return load0(vec2<i32>(in.clip_position.xy));
}

// ---------------------------------------------------------------------------
// Gaussian blur, separable
// ---------------------------------------------------------------------------
//
// p[0] = (direction x, direction y, sigma in pixels, stride in pixels)
// p[1].x = taps on each side
//
// Taps sit at whole multiples of the stride, so with a whole-number stride
// every tap is an exact texel: `fx/reference.rs` reproduces this sum on the
// CPU. A wide blur is sampled sparsely (stride > 1) rather than with hundreds
// of taps; at that width the skipped texels carry nothing a viewer can see.
@fragment
fn fs_blur(in: VertexOutput) -> @location(0) vec4<f32> {
    let dir = vec2<i32>(fx.p[0].xy);
    let sigma = max(fx.p[0].z, 0.0001);
    let stride = i32(fx.p[0].w);
    let taps = i32(fx.p[1].x);
    let centre = vec2<i32>(in.clip_position.xy);
    var sum = vec4<f32>(0.0);
    var total = 0.0;
    for (var k = -taps; k <= taps; k = k + 1) {
        let d = f32(k * stride);
        let w = exp(-(d * d) / (2.0 * sigma * sigma));
        sum = sum + load0(centre + dir * (k * stride)) * w;
        total = total + w;
    }
    return sum / total;
}

// ---------------------------------------------------------------------------
// Downsample with a soft threshold: the bright part of the picture
// ---------------------------------------------------------------------------
//
// p[0] = (threshold 0..1, knee, source: 0 luma 1 red 2 green 3 blue, 0)
//
// Renders at a lower resolution than its input; the bilinear tap at the
// centre of each output pixel averages the input texels under it.
@fragment
fn fs_bright(in: VertexOutput) -> @location(0) vec4<f32> {
    let c = sample0(in.uv);
    var measure: f32;
    switch u32(fx.p[0].z) {
        case 1u: { measure = c.r; }
        case 2u: { measure = c.g; }
        case 3u: { measure = c.b; }
        default: { measure = dot(c.rgb, LUMA); }
    }
    let threshold = fx.p[0].x;
    let knee = max(fx.p[0].y, 0.0001);
    // A smooth ramp from threshold to threshold + knee, so the edge of what
    // glows does not flicker as a value crosses it.
    let k = smoothstep(threshold, threshold + knee, measure);
    return c * k;
}

// Add light: the original plus a blurred bright pass, tinted.
//
// p[0] = (amount, 0, 0, 0)
// p[1] = tint rgb
//
// Light is added, premultiplied. Where the original is transparent the glow
// becomes coverage of its own, so a glowing clip on a letterboxed canvas
// glows into the bars as light would.
@fragment
fn fs_add_light(in: VertexOutput) -> @location(0) vec4<f32> {
    let base = load0(vec2<i32>(in.clip_position.xy));
    let light = sample1(in.uv).rgb * fx.p[1].rgb * fx.p[0].x;
    let rgb = base.rgb + light;
    let coverage = clamp(max(light.r, max(light.g, light.b)), 0.0, 1.0);
    let a = base.a + (1.0 - base.a) * coverage;
    return vec4<f32>(rgb, a);
}

// ---------------------------------------------------------------------------
// Zoom blur
// ---------------------------------------------------------------------------
//
// p[0] = (centre u, centre v, strength 0..1, samples)
//
// Averages samples along the line from the pixel towards the centre, each
// one a step closer: what a zoom during the exposure draws.
@fragment
fn fs_zoom_blur(in: VertexOutput) -> @location(0) vec4<f32> {
    let centre = fx.p[0].xy;
    let strength = fx.p[0].z;
    let n = max(i32(fx.p[0].w), 1);
    var sum = vec4<f32>(0.0);
    for (var i = 0; i < n; i = i + 1) {
        let t = f32(i) / f32(n);
        let uv = centre + (in.uv - centre) * (1.0 - strength * t);
        sum = sum + sample_on(uv);
    }
    return sum / f32(n);
}

// ---------------------------------------------------------------------------
// Affine resample: shake and gate weave
// ---------------------------------------------------------------------------
//
// p[0] = (cos, sin, 1/zoom, 0) of the rotation about the centre
// p[1] = (translation x, translation y) in pixels
//
// The CPU computes the jolt for this instant (`fx/render.rs`, `shake_at`),
// so the shader only resamples, and the CPU reference can predict the image.
@fragment
fn fs_transform(in: VertexOutput) -> @location(0) vec4<f32> {
    let size = fx.frame.xy;
    let px = in.uv * size - size * 0.5 - fx.p[1].xy;
    let c = fx.p[0].x;
    let s = fx.p[0].y;
    // Inverse rotation and zoom: where in the source this pixel came from.
    let src = vec2<f32>(c * px.x + s * px.y, -s * px.x + c * px.y) * fx.p[0].z;
    return sample_on((src + size * 0.5) / size);
}

// ---------------------------------------------------------------------------
// Light sweep
// ---------------------------------------------------------------------------
//
// p[0] = (position along the sweep axis, half width, axis x, axis y)
// p[1] = (colour rgb, intensity)
//
// A soft band of light at `position` on an axis through the centre, in
// units of the frame's half diagonal. Added only where there is picture.
@fragment
fn fs_light_sweep(in: VertexOutput) -> @location(0) vec4<f32> {
    let base = load0(vec2<i32>(in.clip_position.xy));
    let size = fx.frame.xy;
    let half_diag = length(size) * 0.5;
    let d = dot(in.uv * size - size * 0.5, fx.p[0].zw) / half_diag;
    let band = 1.0 - smoothstep(0.0, max(fx.p[0].y, 0.0001), abs(d - fx.p[0].x));
    let light = fx.p[1].rgb * fx.p[1].w * band * band;
    return vec4<f32>(base.rgb + light * base.a, base.a);
}

// ---------------------------------------------------------------------------
// RGB split
// ---------------------------------------------------------------------------
//
// p[0] = (offset x, offset y) in pixels: red moves by it, blue against it.
@fragment
fn fs_rgb_split(in: VertexOutput) -> @location(0) vec4<f32> {
    let o = vec2<i32>(round(fx.p[0].xy));
    let p = vec2<i32>(in.clip_position.xy);
    let r = load0(p - o);
    let g = load0(p);
    let b = load0(p + o);
    return vec4<f32>(r.r, g.g, b.b, max(r.a, max(g.a, b.a)));
}

// ---------------------------------------------------------------------------
// Glitch
// ---------------------------------------------------------------------------
//
// p[0] = (intensity 0..1, band height px, time step, colour shift px)
//
// The frame is cut into horizontal bands whose height varies; on each time
// step a random set of them jumps sideways, and the ones that jump tear
// their red and blue apart. The step, not the time, seeds it, so the picture
// holds still between jumps the way a digital glitch does.
@fragment
fn fs_glitch(in: VertexOutput) -> @location(0) vec4<f32> {
    let p = vec2<i32>(in.clip_position.xy);
    let intensity = fx.p[0].x;
    let band_h = max(fx.p[0].y, 1.0);
    let tick = u32(fx.p[0].z);
    let shift = fx.p[0].w;
    // Band index: coarse bands, each split in two at a random height.
    let coarse = u32(floor(f32(p.y) / band_h));
    let split = hash2(coarse, tick * 7u + 1u);
    let within = fract(f32(p.y) / band_h);
    let band = coarse * 2u + select(0u, 1u, within > split);
    let roll = hash2(band, tick);
    if (roll > intensity * 0.6) {
        return load0(p);
    }
    let jump = (hash2(band, tick + 101u) * 2.0 - 1.0) * fx.frame.x * 0.12 * intensity;
    let q = p + vec2<i32>(i32(jump), 0);
    let tear = vec2<i32>(i32(shift * (0.5 + hash2(band, tick + 7u))), 0);
    let r = load0(q - tear);
    let g = load0(q);
    let b = load0(q + tear);
    return vec4<f32>(r.r, g.g, b.b, max(r.a, max(g.a, b.a)));
}

// ---------------------------------------------------------------------------
// VHS
// ---------------------------------------------------------------------------
//
// p[0] = (intensity, noise, jitter px, scanlines)
// p[1] = (colour bleed px, time step, tracking band position, 0)
//
// Tape in five ingredients: every line wobbles sideways a little, colour is
// smeared along the line (luma stays sharp, as it does on tape), dark
// scanlines, per-pixel noise, and a tracking band that rolls down the frame
// with a stronger wobble in it.
@fragment
fn fs_vhs(in: VertexOutput) -> @location(0) vec4<f32> {
    let p = vec2<i32>(in.clip_position.xy);
    let intensity = fx.p[0].x;
    let tick = u32(fx.p[1].y);
    let row = u32(p.y);
    let h = fx.frame.y;
    // The tracking band: a narrow region with extra displacement.
    let band_y = fx.p[1].z * h;
    let in_band = 1.0 - smoothstep(0.0, h * 0.04, abs(f32(p.y) - band_y));
    let wobble = (hash2(row, tick) * 2.0 - 1.0) * fx.p[0].z * (1.0 + 6.0 * in_band);
    let q = p + vec2<i32>(i32(round(wobble)), 0);
    let centre = load0(q);
    // Chroma bleed: average the colour along the line, keep the centre's luma.
    let bleed = i32(fx.p[1].x);
    var smear = vec4<f32>(0.0);
    var count = 0.0;
    for (var k = 0; k <= 8; k = k + 1) {
        let off = (k * bleed) / 8;
        smear = smear + load0(q - vec2<i32>(off, 0));
        count = count + 1.0;
    }
    smear = smear / count;
    let luma = dot(centre.rgb, LUMA);
    let smear_luma = dot(smear.rgb, LUMA);
    var rgb = smear.rgb + vec3<f32>(luma - smear_luma);
    rgb = mix(centre.rgb, rgb, intensity);
    // Scanlines: every other line darker.
    let line = f32(p.y & 1);
    rgb = rgb * (1.0 - fx.p[0].w * 0.35 * line);
    // Noise, stronger in the tracking band.
    let n = hash2(u32(p.x) + row * 4099u, tick + 3u) * 2.0 - 1.0;
    rgb = rgb + vec3<f32>(n * fx.p[0].y * 0.25 * (1.0 + 2.0 * in_band)) * centre.a;
    // Tape is a little washed out.
    let grey = dot(rgb, LUMA);
    rgb = mix(rgb, vec3<f32>(grey), 0.2 * intensity);
    return vec4<f32>(max(rgb, vec3<f32>(0.0)), centre.a);
}

// ---------------------------------------------------------------------------
// Pixelate
// ---------------------------------------------------------------------------
//
// p[0].x = block size in pixels (whole number)
//
// Every pixel takes the texel at the centre of its block, blocks anchored at
// the centre of the frame so the picture stays centred as the size changes.
@fragment
fn fs_pixelate(in: VertexOutput) -> @location(0) vec4<f32> {
    let block = max(i32(fx.p[0].x), 1);
    let size = vec2<i32>(fx.frame.xy);
    let origin = (size / 2) % vec2<i32>(block);
    let p = vec2<i32>(in.clip_position.xy) - origin;
    let cell = vec2<i32>(floor(vec2<f32>(p) / f32(block)));
    return load0(cell * block + vec2<i32>(block / 2) + origin);
}

// ---------------------------------------------------------------------------
// Mirror
// ---------------------------------------------------------------------------
//
// p[0].x = mode: 0 left onto right, 1 right onto left, 2 top onto bottom,
//          3 bottom onto top, 4 the top-left quarter onto all four.
@fragment
fn fs_mirror(in: VertexOutput) -> @location(0) vec4<f32> {
    let size = vec2<i32>(fx.frame.xy);
    var p = vec2<i32>(in.clip_position.xy);
    let mode = u32(fx.p[0].x);
    let flip = size - vec2<i32>(1) - p;
    switch mode {
        case 0u: { if (p.x >= size.x / 2) { p.x = flip.x; } }
        case 1u: { if (p.x < size.x / 2) { p.x = flip.x; } }
        case 2u: { if (p.y >= size.y / 2) { p.y = flip.y; } }
        case 3u: { if (p.y < size.y / 2) { p.y = flip.y; } }
        default: {
            if (p.x >= size.x / 2) { p.x = flip.x; }
            if (p.y >= size.y / 2) { p.y = flip.y; }
        }
    }
    return load0(p);
}

// ---------------------------------------------------------------------------
// Kaleidoscope
// ---------------------------------------------------------------------------
//
// p[0] = (segments, rotation radians, 1/zoom, 0)
//
// Polar fold: the angle around the centre is folded into one wedge and
// mirrored at every other one, so the edges of neighbouring wedges meet.
@fragment
fn fs_kaleidoscope(in: VertexOutput) -> @location(0) vec4<f32> {
    let size = fx.frame.xy;
    let d = in.uv * size - size * 0.5;
    let r = length(d) * fx.p[0].z;
    let wedge = TAU / max(fx.p[0].x, 1.0);
    var a = atan2(d.y, d.x) + fx.p[0].y;
    a = a - wedge * floor(a / wedge);
    a = abs(a - wedge * 0.5);
    let q = vec2<f32>(cos(a), sin(a)) * r;
    // Sample from the wedge pointing right, which keeps the middle of the
    // frame on screen however the fold turns.
    return sample0((q + size * 0.5) / size);
}

// ---------------------------------------------------------------------------
// Film grain
// ---------------------------------------------------------------------------
//
// p[0] = (amount 0..1, grain size px, time ms, 0)
//
// The grade's grain, as an effect: triangular noise from two PCG hashes per
// grain cell, reseeded every millisecond of clip time, scaling the light.
const GRAIN_STRENGTH: f32 = 0.25;

@fragment
fn fs_grain(in: VertexOutput) -> @location(0) vec4<f32> {
    let c = load0(vec2<i32>(in.clip_position.xy));
    let cell = vec2<u32>(floor(in.clip_position.xy / max(fx.p[0].y, 1.0)));
    let h1 = pcg(cell.x + pcg(cell.y + pcg(u32(fx.p[0].z) + seed_u())));
    let h2 = pcg(h1);
    let n = f32(h1 & 65535u) / 65535.0 + f32(h2 & 65535u) / 65535.0 - 1.0;
    return vec4<f32>(max(c.rgb * (1.0 + n * fx.p[0].x * GRAIN_STRENGTH), vec3<f32>(0.0)), c.a);
}

// ---------------------------------------------------------------------------
// Letterbox
// ---------------------------------------------------------------------------
//
// p[0] = (bar height as a fraction of the frame, bar width as a fraction,
//         opacity, 0)
// p[1] = bar colour, straight alpha
@fragment
fn fs_letterbox(in: VertexOutput) -> @location(0) vec4<f32> {
    let c = load0(vec2<i32>(in.clip_position.xy));
    let bar = in.uv.y < fx.p[0].x || in.uv.y > 1.0 - fx.p[0].x
        || in.uv.x < fx.p[0].y || in.uv.x > 1.0 - fx.p[0].y;
    if (!bar) {
        return c;
    }
    let a = fx.p[1].a * fx.p[0].z;
    let colour = vec4<f32>(fx.p[1].rgb * a, a);
    return colour + c * (1.0 - a);
}

// ---------------------------------------------------------------------------
// Frame: rounded corners, border and drop shadow for picture in picture
// ---------------------------------------------------------------------------
//
// p[0] = (centre x, centre y, x axis x, x axis y), pixels of this frame
// p[1] = (y axis x, y axis y, corner radius, border width), pixels
// p[2] = border colour, straight alpha
// p[3] = shadow colour, straight alpha with the strength folded into a
// p[4] = (shadow offset x, offset y, shadow softness, 0), pixels
//
// The axes are the clip's own edges on screen — half its width and height,
// rotated with it — so the corners round in the clip's frame however it is
// scaled or turned. Everything is a signed distance to a rounded rectangle,
// so the edges are anti-aliased and the shadow needs no blur pass.
fn rounded_box(q: vec2<f32>, half_size: vec2<f32>, radius: f32) -> f32 {
    let r = min(radius, min(half_size.x, half_size.y));
    let d = abs(q) - half_size + vec2<f32>(r);
    return length(max(d, vec2<f32>(0.0))) + min(max(d.x, d.y), 0.0) - r;
}

fn box_local(p: vec2<f32>) -> vec2<f32> {
    let ax = fx.p[0].zw;
    let ay = fx.p[1].xy;
    let d = p - fx.p[0].xy;
    return vec2<f32>(dot(d, ax) / max(length(ax), 0.0001), dot(d, ay) / max(length(ay), 0.0001));
}

@fragment
fn fs_frame(in: VertexOutput) -> @location(0) vec4<f32> {
    let c = load0(vec2<i32>(in.clip_position.xy));
    let p = in.clip_position.xy;
    let half_size = vec2<f32>(length(fx.p[0].zw), length(fx.p[1].xy));
    let radius = fx.p[1].z;
    let border = fx.p[1].w;
    let d = rounded_box(box_local(p), half_size, radius);
    let inside = clamp(0.5 - d, 0.0, 1.0);
    // The border is drawn inside the clip's edge, so it never changes size.
    let ring = clamp(0.5 - (-d - border), 0.0, 1.0) * select(0.0, 1.0, border > 0.0);
    let bc = fx.p[2];
    var clip = c * (1.0 - ring * bc.a) + vec4<f32>(bc.rgb * bc.a, bc.a) * ring;
    clip = clip * inside;
    // The shadow is the same rounded rectangle, moved and softened.
    let sc = fx.p[3];
    let soft = max(fx.p[4].z, 0.5);
    let ds = rounded_box(box_local(p - fx.p[4].xy), half_size, radius);
    let shadow_a = sc.a * (1.0 - smoothstep(-soft, soft, ds));
    let shadow = vec4<f32>(sc.rgb * shadow_a, shadow_a);
    return clip + shadow * (1.0 - clip.a);
}
