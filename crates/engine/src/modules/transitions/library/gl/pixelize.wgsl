// pixelize, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: gre
// License: MIT
// forked from https://gist.github.com/benraziel/c528607361d90a072e98
// minimum number of squares (when the effect is at its higher level)
// zero disable the stepping
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
var<private> squaresMin: vec2<i32>;
var<private> steps: i32;
var<private> d: f32;
var<private> dist: f32;
var<private> squareSize: vec2<f32>;

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
    var local: vec2<f32>;
    var p: vec2<f32>;

    uv_5 = uv_4;
    let _e19 = dist;
    if (_e19 > 0f) {
        let _e22 = uv_5;
        let _e23 = squareSize;
        let _e29 = squareSize;
        local = ((floor((_e22 / _e23)) + vec2(0.5f)) * _e29);
    } else {
        let _e31 = uv_5;
        local = _e31;
    }
    let _e33 = local;
    p = _e33;
    let _e35 = p;
    let _e36 = getFromColor(_e35);
    let _e37 = p;
    let _e38 = getToColor(_e37);
    let _e39 = progress;
    return mix(_e36, _e38, vec4(_e39));
}

fn chukcut_init_globals() {
    var local_1: f32;

    let _e17 = progress;
    let _e19 = progress;
    d = min(_e17, (1f - _e19));
    let _e22 = steps;
    if (_e22 > 0i) {
        let _e25 = d;
        let _e26 = steps;
        let _e30 = steps;
        local_1 = (ceil((_e25 * f32(_e26))) / f32(_e30));
    } else {
        let _e33 = d;
        local_1 = _e33;
    }
    let _e35 = local_1;
    dist = _e35;
    let _e37 = dist;
    let _e39 = squaresMin;
    squareSize = (vec2((2f * _e37)) / vec2<f32>(_e39));
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_2: vec4<f32>;
    var a: f32;
    var local_2: vec4<f32>;

    let _e17 = U;
    progress = _e17.state.x;
    let _e21 = textureDimensions(u_from, 0i);
    dims = vec2<i32>(_e21);
    let _e24 = dims;
    let _e27 = dims;
    ratio = (f32(_e24.x) / f32(max(_e27.y, 1i)));
    let _e36 = U.params[0];
    squaresMin = vec2<i32>(_e36.xy);
    let _e42 = U.params[1];
    steps = i32(_e42.x);
    chukcut_init_globals();
    let _e45 = v_uv_1;
    let _e48 = v_uv_1;
    let _e52 = transition(vec2<f32>(_e45.x, (1f - _e48.y)));
    c_2 = _e52;
    let _e54 = c_2;
    a = clamp(_e54.w, 0f, 1f);
    let _e60 = a;
    if (_e60 > 0.00001f) {
        let _e63 = c_2;
        let _e65 = a;
        let _e67 = (_e63.xyz / vec3(_e65));
        let _e68 = a;
        local_2 = vec4<f32>(_e67.x, _e67.y, _e67.z, _e68);
    } else {
        local_2 = vec4(0f);
    }
    let _e76 = local_2;
    o_color = _e76;
    return;
}

@fragment 
fn main(@location(0) v_uv: vec2<f32>) -> FragmentOutput {
    v_uv_1 = v_uv;
    main_1();
    let _e32 = o_color;
    return FragmentOutput(_e32);
}
