// CrazyParametricFun, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: mandubian
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
var<private> a_1: f32;
var<private> b: f32;
var<private> amplitude: f32;
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

fn transition(uv_4: vec2<f32>) -> vec4<f32> {
    var uv_5: vec2<f32>;
    var p: vec2<f32>;
    var dir: vec2<f32>;
    var dist: f32;
    var x: f32;
    var y: f32;
    var offset: vec2<f32>;

    uv_5 = uv_4;
    let _e18 = uv_5;
    p = (_e18.xy / vec2(1f));
    let _e25 = p;
    dir = (_e25 - vec2(0.5f));
    let _e30 = dir;
    dist = length(_e30);
    let _e33 = a_1;
    let _e34 = b;
    let _e36 = progress;
    let _e39 = b;
    let _e40 = progress;
    let _e41 = a_1;
    let _e42 = b;
    x = (((_e33 - _e34) * cos(_e36)) + (_e39 * cos((_e40 * ((_e41 / _e42) - 1f)))));
    let _e51 = a_1;
    let _e52 = b;
    let _e54 = progress;
    let _e57 = b;
    let _e58 = progress;
    let _e59 = a_1;
    let _e60 = b;
    y = (((_e51 - _e52) * sin(_e54)) - (_e57 * sin((_e58 * ((_e59 / _e60) - 1f)))));
    let _e69 = dir;
    let _e70 = progress;
    let _e71 = dist;
    let _e73 = amplitude;
    let _e75 = x;
    let _e78 = progress;
    let _e79 = dist;
    let _e81 = amplitude;
    let _e83 = y;
    let _e88 = smoothness;
    offset = ((_e69 * vec2<f32>(sin((((_e70 * _e71) * _e73) * _e75)), sin((((_e78 * _e79) * _e81) * _e83)))) / vec2(_e88));
    let _e92 = p;
    let _e93 = offset;
    let _e95 = getFromColor((_e92 + _e93));
    let _e96 = p;
    let _e97 = getToColor(_e96);
    let _e100 = progress;
    return mix(_e95, _e97, vec4(smoothstep(0.2f, 1f, _e100)));
}

fn chukcut_init_globals() {
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_2: vec4<f32>;
    var a: f32;
    var local: vec4<f32>;

    let _e16 = U;
    progress = _e16.state.x;
    let _e20 = textureDimensions(u_from, 0i);
    dims = vec2<i32>(_e20);
    let _e23 = dims;
    let _e26 = dims;
    ratio = (f32(_e23.x) / f32(max(_e26.y, 1i)));
    let _e35 = U.params[0];
    a_1 = _e35.x;
    let _e40 = U.params[1];
    b = _e40.x;
    let _e45 = U.params[2];
    amplitude = _e45.x;
    let _e50 = U.params[3];
    smoothness = _e50.x;
    chukcut_init_globals();
    let _e52 = v_uv_1;
    let _e55 = v_uv_1;
    let _e59 = transition(vec2<f32>(_e52.x, (1f - _e55.y)));
    c_2 = _e59;
    let _e61 = c_2;
    a = clamp(_e61.w, 0f, 1f);
    let _e67 = a;
    if (_e67 > 0.00001f) {
        let _e70 = c_2;
        let _e72 = a;
        let _e74 = (_e70.xyz / vec3(_e72));
        let _e75 = a;
        local = vec4<f32>(_e74.x, _e74.y, _e74.z, _e75);
    } else {
        local = vec4(0f);
    }
    let _e83 = local;
    o_color = _e83;
    return;
}

@fragment 
fn main(@location(0) v_uv: vec2<f32>) -> FragmentOutput {
    v_uv_1 = v_uv;
    main_1();
    let _e30 = o_color;
    return FragmentOutput(_e30);
}
