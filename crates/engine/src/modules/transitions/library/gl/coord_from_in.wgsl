// coord-from-in, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: haiyoucuv
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
    var coordTo: vec4<f32>;
    var coordFrom: vec4<f32>;

    uv_5 = uv_4;
    let _e14 = uv_5;
    let _e15 = getToColor(_e14);
    coordTo = _e15;
    let _e17 = uv_5;
    let _e18 = getFromColor(_e17);
    coordFrom = _e18;
    let _e20 = uv_5;
    let _e21 = coordTo;
    let _e23 = progress;
    let _e26 = getFromColor(mix(_e20, _e21.xy, vec2(_e23)));
    let _e27 = coordFrom;
    let _e29 = uv_5;
    let _e30 = progress;
    let _e33 = getToColor(mix(_e27.xy, _e29, vec2(_e30)));
    let _e34 = progress;
    return mix(_e26, _e33, vec4(_e34));
}

fn chukcut_init_globals() {
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_2: vec4<f32>;
    var a: f32;
    var local: vec4<f32>;

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
        local = vec4<f32>(_e50.x, _e50.y, _e50.z, _e51);
    } else {
        local = vec4(0f);
    }
    let _e59 = local;
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
