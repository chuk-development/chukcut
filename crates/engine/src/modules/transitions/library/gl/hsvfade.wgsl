// HSVfade, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: nwoeanhinnogaehr
// License: MIT
// Ported from https://gist.github.com/nwoeanhinnogaehr/b185145363d65751009b
// HSV functions from http://lolengine.net/blog/2013/07/27/rgb-to-hsv-in-glsl
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

fn hsv2rgb(c_2: vec3<f32>) -> vec3<f32> {
    var c_3: vec3<f32>;
    var K: vec4<f32> = vec4<f32>(1f, 0.6666667f, 0.33333334f, 3f);
    var p: vec3<f32>;

    c_3 = c_2;
    let _e24 = c_3;
    let _e26 = K;
    let _e32 = K;
    p = abs(((fract((_e24.xxx + _e26.xyz)) * 6f) - _e32.www));
    let _e37 = c_3;
    let _e39 = K;
    let _e41 = p;
    let _e42 = K;
    let _e50 = c_3;
    return (_e37.z * mix(_e39.xxx, clamp((_e41 - _e42.xxx), vec3(0f), vec3(1f)), vec3(_e50.y)));
}

fn rgb2hsv(c_4: vec3<f32>) -> vec3<f32> {
    var c_5: vec3<f32>;
    var K_1: vec4<f32> = vec4<f32>(0f, -0.33333334f, 0.6666667f, -1f);
    var p_1: vec4<f32>;
    var q: vec4<f32>;
    var d: f32;

    c_5 = c_4;
    let _e26 = c_5;
    let _e27 = _e26.zy;
    let _e28 = K_1;
    let _e29 = _e28.wz;
    let _e35 = c_5;
    let _e36 = _e35.yz;
    let _e37 = K_1;
    let _e38 = _e37.xy;
    let _e44 = c_5;
    let _e46 = c_5;
    p_1 = mix(vec4<f32>(_e27.x, _e27.y, _e29.x, _e29.y), vec4<f32>(_e36.x, _e36.y, _e38.x, _e38.y), vec4(step(_e44.z, _e46.y)));
    let _e52 = p_1;
    let _e53 = _e52.xyw;
    let _e54 = c_5;
    let _e60 = c_5;
    let _e62 = p_1;
    let _e63 = _e62.yzx;
    let _e68 = p_1;
    let _e70 = c_5;
    q = mix(vec4<f32>(_e53.x, _e53.y, _e53.z, _e54.x), vec4<f32>(_e60.x, _e63.x, _e63.y, _e63.z), vec4(step(_e68.x, _e70.x)));
    let _e76 = q;
    let _e78 = q;
    let _e80 = q;
    d = (_e76.x - min(_e78.w, _e80.y));
    let _e85 = q;
    let _e87 = q;
    let _e89 = q;
    let _e93 = d;
    let _e100 = d;
    let _e101 = q;
    let _e106 = q;
    return vec3<f32>(abs((_e85.z + ((_e87.w - _e89.y) / ((6f * _e93) + 0.001f)))), (_e100 / (_e101.x + 0.001f)), _e106.x);
}

fn transition(uv_4: vec2<f32>) -> vec4<f32> {
    var uv_5: vec2<f32>;
    var a: vec3<f32>;
    var b: vec3<f32>;
    var m: vec3<f32>;

    uv_5 = uv_4;
    let _e14 = uv_5;
    let _e15 = getFromColor(_e14);
    let _e17 = rgb2hsv(_e15.xyz);
    a = _e17;
    let _e19 = uv_5;
    let _e20 = getToColor(_e19);
    let _e22 = rgb2hsv(_e20.xyz);
    b = _e22;
    let _e24 = a;
    let _e25 = b;
    let _e26 = progress;
    m = mix(_e24, _e25, vec3(_e26));
    let _e30 = m;
    let _e31 = hsv2rgb(_e30);
    return vec4<f32>(_e31.x, _e31.y, _e31.z, 1f);
}

fn chukcut_init_globals() {
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_6: vec4<f32>;
    var a_1: f32;
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
    c_6 = _e35;
    let _e37 = c_6;
    a_1 = clamp(_e37.w, 0f, 1f);
    let _e43 = a_1;
    if (_e43 > 0.00001f) {
        let _e46 = c_6;
        let _e48 = a_1;
        let _e50 = (_e46.xyz / vec3(_e48));
        let _e51 = a_1;
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
