// Overexposure, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: Ben Zhang
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

const PI: f32 = 3.1415927f;

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
var<private> strength: f32;

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
    var from_: vec4<f32>;
    var to: vec4<f32>;
    var from_m: f32;
    var to_m: f32;

    uv_5 = uv_4;
    let _e16 = uv_5;
    let _e17 = getFromColor(_e16);
    from_ = _e17;
    let _e19 = uv_5;
    let _e20 = getToColor(_e19);
    to = _e20;
    let _e23 = progress;
    let _e25 = progress;
    let _e28 = strength;
    from_m = ((1f - _e23) + (sin((PI * _e25)) * _e28));
    let _e32 = progress;
    let _e33 = progress;
    let _e36 = strength;
    to_m = (_e32 + (sin((PI * _e33)) * _e36));
    let _e40 = from_;
    let _e42 = from_;
    let _e45 = from_m;
    let _e47 = to;
    let _e49 = to;
    let _e52 = to_m;
    let _e55 = from_;
    let _e57 = from_;
    let _e60 = from_m;
    let _e62 = to;
    let _e64 = to;
    let _e67 = to_m;
    let _e70 = from_;
    let _e72 = from_;
    let _e75 = from_m;
    let _e77 = to;
    let _e79 = to;
    let _e82 = to_m;
    let _e85 = from_;
    let _e87 = to;
    let _e89 = progress;
    return vec4<f32>((((_e40.x * _e42.w) * _e45) + ((_e47.x * _e49.w) * _e52)), (((_e55.y * _e57.w) * _e60) + ((_e62.y * _e64.w) * _e67)), (((_e70.z * _e72.w) * _e75) + ((_e77.z * _e79.w) * _e82)), mix(_e85.w, _e87.w, _e89));
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
    strength = _e33.x;
    chukcut_init_globals();
    let _e35 = v_uv_1;
    let _e38 = v_uv_1;
    let _e42 = transition(vec2<f32>(_e35.x, (1f - _e38.y)));
    c_2 = _e42;
    let _e44 = c_2;
    a = clamp(_e44.w, 0f, 1f);
    let _e50 = a;
    if (_e50 > 0.00001f) {
        let _e53 = c_2;
        let _e55 = a;
        let _e57 = (_e53.xyz / vec3(_e55));
        let _e58 = a;
        local = vec4<f32>(_e57.x, _e57.y, _e57.z, _e58);
    } else {
        local = vec4(0f);
    }
    let _e66 = local;
    o_color = _e66;
    return;
}

@fragment 
fn main(@location(0) v_uv: vec2<f32>) -> FragmentOutput {
    v_uv_1 = v_uv;
    main_1();
    let _e26 = o_color;
    return FragmentOutput(_e26);
}
