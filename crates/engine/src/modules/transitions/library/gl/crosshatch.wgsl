// crosshatch, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: pthrasher
// License: MIT
// adapted by gre from https://gist.github.com/pthrasher/04fd9a7de4012cbb03f6
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
var<private> center: vec2<f32>;
var<private> threshold: f32;
var<private> fadeEdge: f32;

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

fn rand(co: vec2<f32>) -> f32 {
    var co_1: vec2<f32>;

    co_1 = co;
    let _e17 = co_1;
    return fract((sin(dot(_e17.xy, vec2<f32>(12.9898f, 78.233f))) * 43758.547f));
}

fn transition(p: vec2<f32>) -> vec4<f32> {
    var p_1: vec2<f32>;
    var dist: f32;
    var r: f32;

    p_1 = p;
    let _e17 = center;
    let _e18 = p_1;
    let _e20 = threshold;
    dist = (distance(_e17, _e18) / _e20);
    let _e23 = progress;
    let _e24 = p_1;
    let _e28 = rand(vec2<f32>(_e24.y, 0f));
    let _e30 = p_1;
    let _e33 = rand(vec2<f32>(0f, _e30.x));
    r = (_e23 - min(_e28, _e33));
    let _e37 = p_1;
    let _e38 = getFromColor(_e37);
    let _e39 = p_1;
    let _e40 = getToColor(_e39);
    let _e42 = dist;
    let _e43 = r;
    let _e47 = fadeEdge;
    let _e50 = progress;
    let _e54 = fadeEdge;
    let _e55 = progress;
    return mix(_e38, _e40, vec4(mix(0f, mix(step(_e42, _e43), 1f, smoothstep((1f - _e47), 1f, _e50)), smoothstep(0f, _e54, _e55))));
}

fn chukcut_init_globals() {
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_2: vec4<f32>;
    var a: f32;
    var local: vec4<f32>;

    let _e15 = U;
    progress = _e15.state.x;
    let _e19 = textureDimensions(u_from, 0i);
    dims = vec2<i32>(_e19);
    let _e22 = dims;
    let _e25 = dims;
    ratio = (f32(_e22.x) / f32(max(_e25.y, 1i)));
    let _e34 = U.params[0];
    center = _e34.xy;
    let _e39 = U.params[1];
    threshold = _e39.x;
    let _e44 = U.params[2];
    fadeEdge = _e44.x;
    chukcut_init_globals();
    let _e46 = v_uv_1;
    let _e49 = v_uv_1;
    let _e53 = transition(vec2<f32>(_e46.x, (1f - _e49.y)));
    c_2 = _e53;
    let _e55 = c_2;
    a = clamp(_e55.w, 0f, 1f);
    let _e61 = a;
    if (_e61 > 0.00001f) {
        let _e64 = c_2;
        let _e66 = a;
        let _e68 = (_e64.xyz / vec3(_e66));
        let _e69 = a;
        local = vec4<f32>(_e68.x, _e68.y, _e68.z, _e69);
    } else {
        local = vec4(0f);
    }
    let _e77 = local;
    o_color = _e77;
    return;
}

@fragment 
fn main(@location(0) v_uv: vec2<f32>) -> FragmentOutput {
    v_uv_1 = v_uv;
    main_1();
    let _e28 = o_color;
    return FragmentOutput(_e28);
}
