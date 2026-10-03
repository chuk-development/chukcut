// Dreamy, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: mikolalysenko
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
var<private> progress_2: f32;
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

fn offset(progress: f32, x: f32, theta: f32) -> vec2<f32> {
    var progress_1: f32;
    var x_1: f32;
    var theta_1: f32;
    var phase: f32;
    var shifty: f32;

    progress_1 = progress;
    x_1 = x;
    theta_1 = theta;
    let _e18 = progress_1;
    let _e19 = progress_1;
    let _e21 = progress_1;
    let _e23 = theta_1;
    phase = (((_e18 * _e19) + _e21) + _e23);
    let _e27 = progress_1;
    let _e30 = progress_1;
    let _e31 = x_1;
    shifty = ((0.03f * _e27) * cos((10f * (_e30 + _e31))));
    let _e38 = shifty;
    return vec2<f32>(0f, _e38);
}

fn transition(p: vec2<f32>) -> vec4<f32> {
    var p_1: vec2<f32>;

    p_1 = p;
    let _e14 = p_1;
    let _e15 = progress_2;
    let _e16 = p_1;
    let _e19 = offset(_e15, _e16.x, 0f);
    let _e21 = getFromColor((_e14 + _e19));
    let _e22 = p_1;
    let _e24 = progress_2;
    let _e26 = p_1;
    let _e29 = offset((1f - _e24), _e26.x, 3.14f);
    let _e31 = getToColor((_e22 + _e29));
    let _e32 = progress_2;
    return mix(_e21, _e31, vec4(_e32));
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
    progress_2 = _e12.state.x;
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
