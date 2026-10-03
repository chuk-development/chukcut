// circleopen, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
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

const center: vec2<f32> = vec2<f32>(0.5f, 0.5f);
const SQRT_2_: f32 = 1.4142135f;

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
var<private> smoothness: f32;
var<private> opening: bool;

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
    var local: f32;
    var x: f32;
    var m: f32;
    var local_1: f32;

    uv_5 = uv_4;
    let _e18 = opening;
    if _e18 {
        let _e19 = progress;
        local = _e19;
    } else {
        let _e21 = progress;
        local = (1f - _e21);
    }
    let _e24 = local;
    x = _e24;
    let _e26 = smoothness;
    let _e29 = uv_5;
    let _e32 = x;
    let _e34 = smoothness;
    m = smoothstep(-(_e26), 0f, ((SQRT_2_ * distance(center, _e29)) - (_e32 * (1f + _e34))));
    let _e40 = uv_5;
    let _e41 = getFromColor(_e40);
    let _e42 = uv_5;
    let _e43 = getToColor(_e42);
    let _e44 = opening;
    if _e44 {
        let _e46 = m;
        local_1 = (1f - _e46);
    } else {
        let _e48 = m;
        local_1 = _e48;
    }
    let _e50 = local_1;
    return mix(_e41, _e43, vec4(_e50));
}

fn chukcut_init_globals() {
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_2: vec4<f32>;
    var a: f32;
    var local_2: vec4<f32>;

    let _e16 = U;
    progress = _e16.state.x;
    let _e20 = textureDimensions(u_from, 0i);
    dims = vec2<i32>(_e20);
    let _e23 = dims;
    let _e26 = dims;
    ratio = (f32(_e23.x) / f32(max(_e26.y, 1i)));
    let _e35 = U.params[0];
    smoothness = _e35.x;
    let _e40 = U.params[1];
    opening = (_e40.x > 0.5f);
    chukcut_init_globals();
    let _e44 = v_uv_1;
    let _e47 = v_uv_1;
    let _e51 = transition(vec2<f32>(_e44.x, (1f - _e47.y)));
    c_2 = _e51;
    let _e53 = c_2;
    a = clamp(_e53.w, 0f, 1f);
    let _e59 = a;
    if (_e59 > 0.00001f) {
        let _e62 = c_2;
        let _e64 = a;
        let _e66 = (_e62.xyz / vec3(_e64));
        let _e67 = a;
        local_2 = vec4<f32>(_e66.x, _e66.y, _e66.z, _e67);
    } else {
        local_2 = vec4(0f);
    }
    let _e75 = local_2;
    o_color = _e75;
    return;
}

@fragment 
fn main(@location(0) v_uv: vec2<f32>) -> FragmentOutput {
    v_uv_1 = v_uv;
    main_1();
    let _e30 = o_color;
    return FragmentOutput(_e30);
}
