// windowslice, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: gre
// License: MIT
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
var<private> count: f32;
var<private> smoothness: f32;

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
    var pr: f32;
    var s: f32;

    p_1 = p;
    let _e16 = smoothness;
    let _e19 = p_1;
    let _e21 = progress;
    let _e23 = smoothness;
    pr = smoothstep(-(_e16), 0f, (_e19.x - (_e21 * (1f + _e23))));
    let _e29 = pr;
    let _e30 = count;
    let _e31 = p_1;
    s = step(_e29, fract((_e30 * _e31.x)));
    let _e37 = p_1;
    let _e38 = getFromColor(_e37);
    let _e39 = p_1;
    let _e40 = getToColor(_e39);
    let _e41 = s;
    return mix(_e38, _e40, vec4(_e41));
}

fn chukcut_init_globals() {
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_2: vec4<f32>;
    var a: f32;
    var local: vec4<f32>;

    let _e14 = U;
    progress = _e14.state.x;
    let _e18 = textureDimensions(u_from, 0i);
    dims = vec2<i32>(_e18);
    let _e21 = dims;
    let _e24 = dims;
    ratio = (f32(_e21.x) / f32(max(_e24.y, 1i)));
    let _e33 = U.params[0];
    count = _e33.x;
    let _e38 = U.params[1];
    smoothness = _e38.x;
    chukcut_init_globals();
    let _e40 = v_uv_1;
    let _e43 = v_uv_1;
    let _e47 = transition(vec2<f32>(_e40.x, (1f - _e43.y)));
    c_2 = _e47;
    let _e49 = c_2;
    a = clamp(_e49.w, 0f, 1f);
    let _e55 = a;
    if (_e55 > 0.00001f) {
        let _e58 = c_2;
        let _e60 = a;
        let _e62 = (_e58.xyz / vec3(_e60));
        let _e63 = a;
        local = vec4<f32>(_e62.x, _e62.y, _e62.z, _e63);
    } else {
        local = vec4(0f);
    }
    let _e71 = local;
    o_color = _e71;
    return;
}

@fragment 
fn main(@location(0) v_uv: vec2<f32>) -> FragmentOutput {
    v_uv_1 = v_uv;
    main_1();
    let _e26 = o_color;
    return FragmentOutput(_e26);
}
