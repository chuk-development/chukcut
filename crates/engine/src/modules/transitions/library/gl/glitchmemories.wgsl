// GlitchMemories, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: Gunnar Roth
// License: MIT
// based on work from natewave
//
// Translated from GLSL to WGSL by naga 30 through the harness in
// `../port.py`; the licence texts are in `../LICENSE-gl-transitions.md`.
// Edit `port.py`, not this file.
struct GlBlock {
    state: vec4<f32>,
    params: array<vec4<f32>, 12>,
}

struct FragmentOutput {
    @location(0) o_color: vec4<f32>,
}

var<private> v_uv_1: vec2<f32>;
var<private> o_color: vec4<f32>;
@group(0) @binding(0) 
var<uniform> U: GlBlock;
@group(1) @binding(0) 
var u_from: texture_2d<f32>;
@group(1) @binding(1) 
var u_to: texture_2d<f32>;
@group(1) @binding(2) 
var u_sampler: sampler;
var<private> progress: f32;
var<private> ratio: f32;

fn chukcut_premultiply(c: vec4<f32>) -> vec4<f32> {
    var c_1: vec4<f32>;

    c_1 = c;
    let _e12 = c_1;
    let _e14 = c_1;
    let _e16 = (_e12.xyz * _e14.w);
    let _e17 = c_1;
    return vec4<f32>(_e16.x, _e16.y, _e16.z, _e17.w);
}

fn getFromColor(uv: vec2<f32>) -> vec4<f32> {
    var uv_1: vec2<f32>;

    uv_1 = uv;
    let _e12 = uv_1;
    let _e15 = uv_1;
    let _e19 = textureSample(u_from, u_sampler, vec2<f32>(_e12.x, (1f - _e15.y)));
    let _e20 = chukcut_premultiply(_e19);
    return _e20;
}

fn getToColor(uv_2: vec2<f32>) -> vec4<f32> {
    var uv_3: vec2<f32>;

    uv_3 = uv_2;
    let _e12 = uv_3;
    let _e15 = uv_3;
    let _e19 = textureSample(u_to, u_sampler, vec2<f32>(_e12.x, (1f - _e15.y)));
    let _e20 = chukcut_premultiply(_e19);
    return _e20;
}

fn transition(p: vec2<f32>) -> vec4<f32> {
    var p_1: vec2<f32>;
    var block: vec2<f32>;
    var uv_noise: vec2<f32>;
    var local: vec2<f32>;
    var dist: vec2<f32>;
    var red: vec2<f32>;
    var green: vec2<f32>;
    var blue: vec2<f32>;

    p_1 = p;
    let _e14 = p_1;
    block = floor((_e14.xy / vec2(16f)));
    let _e22 = block;
    uv_noise = (_e22 / vec2(64f));
    let _e28 = uv_noise;
    let _e29 = progress;
    uv_noise = (_e28 + (floor((vec2(_e29) * vec2<f32>(1200f, 3500f))) / vec2(64f)));
    let _e41 = progress;
    if (_e41 > 0f) {
        let _e44 = uv_noise;
        let _e52 = progress;
        local = (((fract(_e44) - vec2(0.5f)) * 0.3f) * (1f - _e52));
    } else {
        local = vec2(0f);
    }
    let _e58 = local;
    dist = _e58;
    let _e60 = p_1;
    let _e61 = dist;
    red = (_e60 + (_e61 * 0.2f));
    let _e66 = p_1;
    let _e67 = dist;
    green = (_e66 + (_e67 * 0.3f));
    let _e72 = p_1;
    let _e73 = dist;
    blue = (_e72 + (_e73 * 0.5f));
    let _e78 = red;
    let _e79 = getFromColor(_e78);
    let _e80 = red;
    let _e81 = getToColor(_e80);
    let _e82 = progress;
    let _e86 = green;
    let _e87 = getFromColor(_e86);
    let _e88 = green;
    let _e89 = getToColor(_e88);
    let _e90 = progress;
    let _e94 = blue;
    let _e95 = getFromColor(_e94);
    let _e96 = blue;
    let _e97 = getToColor(_e96);
    let _e98 = progress;
    return vec4<f32>(mix(_e79, _e81, vec4(_e82)).x, mix(_e87, _e89, vec4(_e90)).y, mix(_e95, _e97, vec4(_e98)).z, 1f);
}

fn chukcut_init_globals() {
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_2: vec4<f32>;
    var a: f32;
    var local_1: vec4<f32>;

    let _e12 = U;
    progress = _e12.state.x;
    let _e16 = textureDimensions(u_from, 0i);
    dims = vec2<i32>(_e16);
    let _e19 = dims;
    let _e22 = dims;
    ratio = (f32(_e19.x) / f32(max(_e22.y, 1i)));
    chukcut_init_globals();
    let _e28 = v_uv_1;
    let _e31 = v_uv_1;
    let _e35 = transition(vec2<f32>(_e28.x, (1f - _e31.y)));
    c_2 = _e35;
    let _e37 = c_2;
    a = clamp(_e37.w, 0f, 1f);
    let _e43 = a;
    if (_e43 > 0.00001f) {
        let _e46 = c_2;
        let _e48 = a;
        let _e50 = (_e46.xyz / vec3(_e48));
        let _e51 = a;
        local_1 = vec4<f32>(_e50.x, _e50.y, _e50.z, _e51);
    } else {
        local_1 = vec4(0f);
    }
    let _e59 = local_1;
    o_color = _e59;
    return;
}

@fragment 
fn main(@location(0) v_uv: vec2<f32>) -> FragmentOutput {
    v_uv_1 = v_uv;
    main_1();
    let _e22 = o_color;
    return FragmentOutput(_e22);
}
