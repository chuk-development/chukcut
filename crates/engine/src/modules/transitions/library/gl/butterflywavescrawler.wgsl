// ButterflyWaveScrawler, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
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
var<private> progress_2: f32;
var<private> ratio: f32;
var<private> amplitude: f32;
var<private> waves: f32;
var<private> colorSeparation: f32;

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

fn compute(p: vec2<f32>, progress: f32, center: vec2<f32>) -> f32 {
    var p_1: vec2<f32>;
    var progress_1: f32;
    var center_1: vec2<f32>;
    var o: vec2<f32>;
    var h: vec2<f32> = vec2<f32>(1f, 0f);
    var theta: f32;
    var s: f32;
    var s2_: f32;

    p_1 = p;
    progress_1 = progress;
    center_1 = center;
    let _e22 = p_1;
    let _e23 = progress_1;
    let _e24 = amplitude;
    let _e28 = center_1;
    o = ((_e22 * sin((_e23 * _e24))) - _e28);
    let _e35 = o;
    let _e36 = h;
    let _e39 = waves;
    theta = (acos(dot(_e35, _e36)) * _e39);
    let _e43 = theta;
    s = sin((((2f * _e43) - PI) / 24f));
    let _e50 = s;
    let _e51 = s;
    s2_ = (_e50 * _e51);
    let _e54 = theta;
    let _e59 = theta;
    let _e64 = s2_;
    let _e65 = s2_;
    let _e67 = s;
    return (((exp(cos(_e54)) - (2f * cos((4f * _e59)))) + ((_e64 * _e65) * _e67)) / 10f);
}

fn transition(uv_4: vec2<f32>) -> vec4<f32> {
    var uv_5: vec2<f32>;
    var p_2: vec2<f32>;
    var inv: f32;
    var disp: f32;
    var texTo: vec4<f32>;
    var texFrom: vec4<f32>;

    uv_5 = uv_4;
    let _e18 = progress_2;
    if (_e18 <= 0f) {
        let _e21 = uv_5;
        let _e22 = getFromColor(_e21);
        return _e22;
    }
    let _e23 = progress_2;
    if (_e23 >= 1f) {
        let _e26 = uv_5;
        let _e27 = getToColor(_e26);
        return _e27;
    }
    let _e28 = uv_5;
    p_2 = _e28;
    let _e31 = progress_2;
    inv = (1f - _e31);
    let _e34 = p_2;
    let _e35 = progress_2;
    let _e39 = compute(_e34, _e35, vec2<f32>(0.5f, 0.5f));
    disp = _e39;
    let _e41 = p_2;
    let _e42 = inv;
    let _e43 = disp;
    let _e47 = getToColor((_e41 + vec2((_e42 * _e43))));
    texTo = _e47;
    let _e49 = p_2;
    let _e50 = progress_2;
    let _e51 = disp;
    let _e54 = colorSeparation;
    let _e59 = getFromColor((_e49 + vec2(((_e50 * _e51) * (1f - _e54)))));
    let _e61 = p_2;
    let _e62 = progress_2;
    let _e63 = disp;
    let _e67 = getFromColor((_e61 + vec2((_e62 * _e63))));
    let _e69 = p_2;
    let _e70 = progress_2;
    let _e71 = disp;
    let _e74 = colorSeparation;
    let _e79 = getFromColor((_e69 + vec2(((_e70 * _e71) * (1f + _e74)))));
    texFrom = vec4<f32>(_e59.x, _e67.y, _e79.z, 1f);
    let _e84 = texTo;
    let _e85 = progress_2;
    let _e87 = texFrom;
    let _e88 = inv;
    return ((_e84 * _e85) + (_e87 * _e88));
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
    progress_2 = _e16.state.x;
    let _e20 = textureDimensions(u_from, 0i);
    dims = vec2<i32>(_e20);
    let _e23 = dims;
    let _e26 = dims;
    ratio = (f32(_e23.x) / f32(max(_e26.y, 1i)));
    let _e35 = U.params[0];
    amplitude = _e35.x;
    let _e40 = U.params[1];
    waves = _e40.x;
    let _e45 = U.params[2];
    colorSeparation = _e45.x;
    chukcut_init_globals();
    let _e47 = v_uv_1;
    let _e50 = v_uv_1;
    let _e54 = transition(vec2<f32>(_e47.x, (1f - _e50.y)));
    c_2 = _e54;
    let _e56 = c_2;
    a = clamp(_e56.w, 0f, 1f);
    let _e62 = a;
    if (_e62 > 0.00001f) {
        let _e65 = c_2;
        let _e67 = a;
        let _e69 = (_e65.xyz / vec3(_e67));
        let _e70 = a;
        local = vec4<f32>(_e69.x, _e69.y, _e69.z, _e70);
    } else {
        local = vec4(0f);
    }
    let _e78 = local;
    o_color = _e78;
    return;
}

@fragment 
fn main(@location(0) v_uv: vec2<f32>) -> FragmentOutput {
    v_uv_1 = v_uv;
    main_1();
    let _e30 = o_color;
    return FragmentOutput(_e30);
}
