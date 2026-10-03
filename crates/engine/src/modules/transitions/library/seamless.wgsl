// Seamless short-form transitions: both clips move as one, with motion blur.
//
// chukcut's own, written from the description of the technique (a camera
// move across a cut, hidden by its own blur), not from anyone's code. They
// share the library's uniform block and layer bindings with the ported
// gl-transitions; see `mod.rs`.
//
// state = (progress, unused, direction, motion blur)
//   progress   eased, 0..1
//   direction  0 left, 1 right, 2 up, 3 down (`TransitionDirection`)
//   blur       0 none .. 1 full
// params[0].x = strength: zoom factor, turns.
//
// ## How it reads as one move
//
// The outgoing clip carries the move through the first half and the incoming
// clip finishes it in the second, so the cut lands at the fastest instant,
// where the motion blur is widest and hides it. Blur is a shutter: each pixel
// averages the move at sixteen instants around the current one, and the
// shutter's width follows the speed of an ease-in-out move — nothing at the
// ends, widest in the middle — so the transition starts and ends on a sharp
// frame.
//
// Off-frame samples mirror back into the picture rather than showing the
// background, which is what lets a zoom out or a whip pan stay full frame.
//
// All arithmetic is in premultiplied linear light, like every other
// transition: straight-alpha layers in, straight alpha out.

struct GlBlock {
    state: vec4<f32>,
    params: array<vec4<f32>, 12>,
};

@group(0) @binding(0) var<uniform> U: GlBlock;
@group(1) @binding(0) var u_from: texture_2d<f32>;
@group(1) @binding(1) var u_to: texture_2d<f32>;
@group(1) @binding(2) var u_sampler: sampler;

const TAPS: i32 = 16;
const PI: f32 = 3.14159265;

fn premultiply(c: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(c.rgb * c.a, c.a);
}

fn unpremultiply(c: vec4<f32>) -> vec4<f32> {
    if (c.a <= 0.00001) {
        return vec4<f32>(0.0);
    }
    return vec4<f32>(c.rgb / c.a, c.a);
}

// A triangle wave: 0..1 maps to itself, beyond it reflects back.
fn mirror(uv: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(1.0) - abs(vec2<f32>(1.0) - fract(uv * 0.5) * 2.0);
}

fn from_at(uv: vec2<f32>) -> vec4<f32> {
    return premultiply(textureSampleLevel(u_from, u_sampler, mirror(uv), 0.0));
}

fn to_at(uv: vec2<f32>) -> vec4<f32> {
    return premultiply(textureSampleLevel(u_to, u_sampler, mirror(uv), 0.0));
}

// The instant of tap `i` for a frame at progress `t`.
fn shutter(t: f32, i: i32) -> f32 {
    let width = U.state.w * 0.16 * sin(PI * clamp(t, 0.0, 1.0));
    return clamp(t + (f32(i) / f32(TAPS - 1) - 0.5) * width, 0.0, 0.9999);
}

fn aspect() -> f32 {
    let dims = vec2<f32>(textureDimensions(u_from, 0));
    return dims.x / max(dims.y, 1.0);
}

// The unit vector the picture travels along, in UV (y down).
fn travel() -> vec2<f32> {
    switch u32(U.state.z) {
        case 0u: { return vec2<f32>(-1.0, 0.0); }
        case 1u: { return vec2<f32>(1.0, 0.0); }
        case 2u: { return vec2<f32>(0.0, -1.0); }
        default: { return vec2<f32>(0.0, 1.0); }
    }
}

// Where `uv` samples a picture scaled by `s` about the centre.
fn scaled(uv: vec2<f32>, s: f32) -> vec2<f32> {
    return vec2<f32>(0.5) + (uv - vec2<f32>(0.5)) / s;
}

// Where `uv` samples a picture turned by `angle` about the centre, measured
// in pixels so a turn is round on any canvas.
fn turned(uv: vec2<f32>, angle: f32) -> vec2<f32> {
    let r = aspect();
    let d = (uv - vec2<f32>(0.5)) * vec2<f32>(r, 1.0);
    let c = cos(angle);
    let s = sin(angle);
    let q = vec2<f32>(c * d.x + s * d.y, -s * d.x + c * d.y);
    return vec2<f32>(0.5) + q / vec2<f32>(r, 1.0);
}

// Zoom in through: the outgoing clip pushes in, the incoming one arrives
// already pushing in and settles at full frame.
fn zoom_in_at(uv: vec2<f32>, t: f32) -> vec4<f32> {
    let amount = max(U.params[0].x, 1.01);
    if (t < 0.5) {
        return from_at(scaled(uv, pow(amount, t * 2.0)));
    }
    return to_at(scaled(uv, pow(amount, t * 2.0 - 2.0)));
}

// Zoom out through: the mirror image of zoom in.
fn zoom_out_at(uv: vec2<f32>, t: f32) -> vec4<f32> {
    let amount = max(U.params[0].x, 1.01);
    if (t < 0.5) {
        return from_at(scaled(uv, pow(amount, -t * 2.0)));
    }
    return to_at(scaled(uv, pow(amount, 2.0 - t * 2.0)));
}

// Spin: the outgoing clip turns away and the incoming one turns in, the same
// way, `turns` half turns in all.
fn spin_at(uv: vec2<f32>, t: f32) -> vec4<f32> {
    let total = U.params[0].x * PI;
    if (t < 0.5) {
        return from_at(turned(uv, t * total));
    }
    return to_at(turned(uv, (t - 1.0) * total));
}

// Whip pan: both clips fly past in the direction of travel, one frame width
// each.
fn whip_at(uv: vec2<f32>, t: f32) -> vec4<f32> {
    let d = travel();
    if (t < 0.5) {
        return from_at(uv - d * (t * 2.0));
    }
    return to_at(uv + d * (2.0 - t * 2.0));
}

// Push: the incoming clip shoves the outgoing one out, edge to edge.
fn push_at(uv: vec2<f32>, t: f32) -> vec4<f32> {
    let d = travel();
    let a = uv - d * t;
    if (a.x >= 0.0 && a.x <= 1.0 && a.y >= 0.0 && a.y <= 1.0) {
        return premultiply(textureSampleLevel(u_from, u_sampler, a, 0.0));
    }
    return premultiply(textureSampleLevel(u_to, u_sampler, uv + d * (1.0 - t), 0.0));
}

@fragment
fn fs_zoom_in(@location(0) v_uv: vec2<f32>) -> @location(0) vec4<f32> {
    var sum = vec4<f32>(0.0);
    for (var i = 0; i < TAPS; i = i + 1) {
        sum = sum + zoom_in_at(v_uv, shutter(U.state.x, i));
    }
    return unpremultiply(sum / f32(TAPS));
}

@fragment
fn fs_zoom_out(@location(0) v_uv: vec2<f32>) -> @location(0) vec4<f32> {
    var sum = vec4<f32>(0.0);
    for (var i = 0; i < TAPS; i = i + 1) {
        sum = sum + zoom_out_at(v_uv, shutter(U.state.x, i));
    }
    return unpremultiply(sum / f32(TAPS));
}

@fragment
fn fs_spin(@location(0) v_uv: vec2<f32>) -> @location(0) vec4<f32> {
    var sum = vec4<f32>(0.0);
    for (var i = 0; i < TAPS; i = i + 1) {
        sum = sum + spin_at(v_uv, shutter(U.state.x, i));
    }
    return unpremultiply(sum / f32(TAPS));
}

@fragment
fn fs_whip(@location(0) v_uv: vec2<f32>) -> @location(0) vec4<f32> {
    var sum = vec4<f32>(0.0);
    for (var i = 0; i < TAPS; i = i + 1) {
        sum = sum + whip_at(v_uv, shutter(U.state.x, i));
    }
    return unpremultiply(sum / f32(TAPS));
}

@fragment
fn fs_push(@location(0) v_uv: vec2<f32>) -> @location(0) vec4<f32> {
    var sum = vec4<f32>(0.0);
    for (var i = 0; i < TAPS; i = i + 1) {
        sum = sum + push_at(v_uv, shutter(U.state.x, i));
    }
    return unpremultiply(sum / f32(TAPS));
}
