// morph, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: paniq
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

fn transition(p: vec2<f32>) -> vec4<f32> {
    var p_1: vec2<f32>;
    var ca: vec4<f32>;
    var cb: vec4<f32>;
    var oa: vec2<f32>;
    var ob: vec2<f32>;
    var oc: vec2<f32>;
    var w0_: f32;
    var w1_: f32;

    p_1 = p;
    let _e15 = p_1;
    let _e16 = getFromColor(_e15);
    ca = _e16;
    let _e18 = p_1;
    let _e19 = getToColor(_e18);
    cb = _e19;
    let _e21 = ca;
    let _e23 = ca;
    oa = ((((_e21.xy + vec2(_e23.z)) * 0.5f) * 2f) - vec2(1f));
    let _e35 = cb;
    let _e37 = cb;
    ob = ((((_e35.xy + vec2(_e37.z)) * 0.5f) * 2f) - vec2(1f));
    let _e49 = oa;
    let _e50 = ob;
    let _e54 = strength;
    oc = (mix(_e49, _e50, vec2(0.5f)) * _e54);
    let _e57 = progress;
    w0_ = _e57;
    let _e60 = w0_;
    w1_ = (1f - _e60);
    let _e63 = p_1;
    let _e64 = oc;
    let _e65 = w0_;
    let _e68 = getFromColor((_e63 + (_e64 * _e65)));
    let _e69 = p_1;
    let _e70 = oc;
    let _e71 = w1_;
    let _e74 = getToColor((_e69 - (_e70 * _e71)));
    let _e75 = progress;
    return mix(_e68, _e74, vec4(_e75));
}

fn chukcut_init_globals() {
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_2: vec4<f32>;
    var a: f32;
    var local: vec4<f32>;

    let _e13 = U;
    progress = _e13.state.x;
    let _e17 = textureDimensions(u_from, 0i);
    dims = vec2<i32>(_e17);
    let _e20 = dims;
    let _e23 = dims;
    ratio = (f32(_e20.x) / f32(max(_e23.y, 1i)));
    let _e32 = U.params[0];
    strength = _e32.x;
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
        local = vec4<f32>(_e56.x, _e56.y, _e56.z, _e57);
    } else {
        local = vec4(0f);
    }
    let _e65 = local;
    o_color = _e65;
    return;
}

@fragment 
fn main(@location(0) v_uv: vec2<f32>) -> FragmentOutput {
    v_uv_1 = v_uv;
    main_1();
    let _e24 = o_color;
    return FragmentOutput(_e24);
}
