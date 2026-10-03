// Rectangle, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: martiniti
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
var<private> bgcolor: vec4<f32>;
var<private> s: f32;

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
    var sq: vec2<f32>;
    var bl: vec2<f32>;
    var dist: f32;
    var tr: vec2<f32>;
    var local: vec4<f32>;

    p_1 = p;
    let _e16 = p_1;
    sq = (_e16.xy / vec2(1f));
    let _e25 = progress;
    let _e30 = sq;
    bl = step(vec2(abs((1f - (2f * _e25)))), (_e30 + vec2(0.25f)));
    let _e36 = bl;
    let _e38 = bl;
    dist = (_e36.x * _e38.y);
    let _e44 = progress;
    let _e50 = sq;
    tr = step(vec2(abs((1f - (2f * _e44)))), (vec2(1.25f) - _e50));
    let _e55 = dist;
    let _e57 = tr;
    let _e60 = tr;
    dist = (_e55 * ((1f * _e57.x) * _e60.y));
    let _e64 = progress;
    if (_e64 < 0.5f) {
        let _e67 = p_1;
        let _e68 = getFromColor(_e67);
        local = _e68;
    } else {
        let _e69 = p_1;
        let _e70 = getToColor(_e69);
        local = _e70;
    }
    let _e72 = local;
    let _e73 = bgcolor;
    let _e74 = s;
    let _e75 = dist;
    return mix(_e72, _e73, vec4(step(_e74, _e75)));
}

fn chukcut_init_globals() {
    let _e15 = progress;
    s = pow((2f * abs((_e15 - 0.5f))), 3f);
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_2: vec4<f32>;
    var a: f32;
    var local_1: vec4<f32>;

    let _e14 = U;
    progress = _e14.state.x;
    let _e18 = textureDimensions(u_from, 0i);
    dims = vec2<i32>(_e18);
    let _e21 = dims;
    let _e24 = dims;
    ratio = (f32(_e21.x) / f32(max(_e24.y, 1i)));
    let _e33 = U.params[0];
    bgcolor = _e33;
    chukcut_init_globals();
    let _e34 = v_uv_1;
    let _e37 = v_uv_1;
    let _e41 = transition(vec2<f32>(_e34.x, (1f - _e37.y)));
    c_2 = _e41;
    let _e43 = c_2;
    a = clamp(_e43.w, 0f, 1f);
    let _e49 = a;
    if (_e49 > 0.00001f) {
        let _e52 = c_2;
        let _e54 = a;
        let _e56 = (_e52.xyz / vec3(_e54));
        let _e57 = a;
        local_1 = vec4<f32>(_e56.x, _e56.y, _e56.z, _e57);
    } else {
        local_1 = vec4(0f);
    }
    let _e65 = local_1;
    o_color = _e65;
    return;
}

@fragment 
fn main(@location(0) v_uv: vec2<f32>) -> FragmentOutput {
    v_uv_1 = v_uv;
    main_1();
    let _e26 = o_color;
    return FragmentOutput(_e26);
}
