// DefocusBlur, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: Sergey Kosarevsky
// License: MIT
// Ported from https://gist.github.com/corporateshark/b9f8e5675c647e615419
// 12-tap Poisson disk
// https://github.com/spite/Wagner/blob/master/fragment-shaders/poisson-disc-blur-fs.glsl
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
var<private> blurSize: f32;

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
    var T: f32;
    var half: f32 = 0.5f;
    var local: f32;
    var D: f32;
    var C0_: vec4<f32>;
    var C1_: vec4<f32>;

    uv_5 = uv_4;
    let _e15 = progress;
    T = _e15;
    let _e19 = T;
    let _e20 = half;
    if (_e19 < _e20) {
        let _e23 = blurSize;
        let _e24 = T;
        let _e25 = half;
        local = mix(0f, _e23, (_e24 / _e25));
    } else {
        let _e28 = blurSize;
        let _e30 = T;
        let _e31 = half;
        let _e33 = half;
        local = mix(_e28, 0f, ((_e30 - _e31) / _e33));
    }
    let _e37 = local;
    D = _e37;
    let _e39 = uv_5;
    let _e40 = getFromColor(_e39);
    C0_ = _e40;
    let _e42 = uv_5;
    let _e43 = getToColor(_e42);
    C1_ = _e43;
    let _e45 = C0_;
    let _e51 = D;
    let _e53 = uv_5;
    let _e55 = getFromColor(((vec2<f32>(-0.326f, -0.406f) * _e51) + _e53));
    C0_ = (_e45 + _e55);
    let _e57 = C1_;
    let _e63 = D;
    let _e65 = uv_5;
    let _e67 = getToColor(((vec2<f32>(-0.326f, -0.406f) * _e63) + _e65));
    C1_ = (_e57 + _e67);
    let _e69 = C0_;
    let _e75 = D;
    let _e77 = uv_5;
    let _e79 = getFromColor(((vec2<f32>(-0.84f, -0.074f) * _e75) + _e77));
    C0_ = (_e69 + _e79);
    let _e81 = C1_;
    let _e87 = D;
    let _e89 = uv_5;
    let _e91 = getToColor(((vec2<f32>(-0.84f, -0.074f) * _e87) + _e89));
    C1_ = (_e81 + _e91);
    let _e93 = C0_;
    let _e98 = D;
    let _e100 = uv_5;
    let _e102 = getFromColor(((vec2<f32>(-0.696f, 0.457f) * _e98) + _e100));
    C0_ = (_e93 + _e102);
    let _e104 = C1_;
    let _e109 = D;
    let _e111 = uv_5;
    let _e113 = getToColor(((vec2<f32>(-0.696f, 0.457f) * _e109) + _e111));
    C1_ = (_e104 + _e113);
    let _e115 = C0_;
    let _e120 = D;
    let _e122 = uv_5;
    let _e124 = getFromColor(((vec2<f32>(-0.203f, 0.621f) * _e120) + _e122));
    C0_ = (_e115 + _e124);
    let _e126 = C1_;
    let _e131 = D;
    let _e133 = uv_5;
    let _e135 = getToColor(((vec2<f32>(-0.203f, 0.621f) * _e131) + _e133));
    C1_ = (_e126 + _e135);
    let _e137 = C0_;
    let _e142 = D;
    let _e144 = uv_5;
    let _e146 = getFromColor(((vec2<f32>(0.962f, -0.195f) * _e142) + _e144));
    C0_ = (_e137 + _e146);
    let _e148 = C1_;
    let _e153 = D;
    let _e155 = uv_5;
    let _e157 = getToColor(((vec2<f32>(0.962f, -0.195f) * _e153) + _e155));
    C1_ = (_e148 + _e157);
    let _e159 = C0_;
    let _e164 = D;
    let _e166 = uv_5;
    let _e168 = getFromColor(((vec2<f32>(0.473f, -0.48f) * _e164) + _e166));
    C0_ = (_e159 + _e168);
    let _e170 = C1_;
    let _e175 = D;
    let _e177 = uv_5;
    let _e179 = getToColor(((vec2<f32>(0.473f, -0.48f) * _e175) + _e177));
    C1_ = (_e170 + _e179);
    let _e181 = C0_;
    let _e185 = D;
    let _e187 = uv_5;
    let _e189 = getFromColor(((vec2<f32>(0.519f, 0.767f) * _e185) + _e187));
    C0_ = (_e181 + _e189);
    let _e191 = C1_;
    let _e195 = D;
    let _e197 = uv_5;
    let _e199 = getToColor(((vec2<f32>(0.519f, 0.767f) * _e195) + _e197));
    C1_ = (_e191 + _e199);
    let _e201 = C0_;
    let _e206 = D;
    let _e208 = uv_5;
    let _e210 = getFromColor(((vec2<f32>(0.185f, -0.893f) * _e206) + _e208));
    C0_ = (_e201 + _e210);
    let _e212 = C1_;
    let _e217 = D;
    let _e219 = uv_5;
    let _e221 = getToColor(((vec2<f32>(0.185f, -0.893f) * _e217) + _e219));
    C1_ = (_e212 + _e221);
    let _e223 = C0_;
    let _e227 = D;
    let _e229 = uv_5;
    let _e231 = getFromColor(((vec2<f32>(0.507f, 0.064f) * _e227) + _e229));
    C0_ = (_e223 + _e231);
    let _e233 = C1_;
    let _e237 = D;
    let _e239 = uv_5;
    let _e241 = getToColor(((vec2<f32>(0.507f, 0.064f) * _e237) + _e239));
    C1_ = (_e233 + _e241);
    let _e243 = C0_;
    let _e247 = D;
    let _e249 = uv_5;
    let _e251 = getFromColor(((vec2<f32>(0.896f, 0.412f) * _e247) + _e249));
    C0_ = (_e243 + _e251);
    let _e253 = C1_;
    let _e257 = D;
    let _e259 = uv_5;
    let _e261 = getToColor(((vec2<f32>(0.896f, 0.412f) * _e257) + _e259));
    C1_ = (_e253 + _e261);
    let _e263 = C0_;
    let _e269 = D;
    let _e271 = uv_5;
    let _e273 = getFromColor(((vec2<f32>(-0.322f, -0.933f) * _e269) + _e271));
    C0_ = (_e263 + _e273);
    let _e275 = C1_;
    let _e281 = D;
    let _e283 = uv_5;
    let _e285 = getToColor(((vec2<f32>(-0.322f, -0.933f) * _e281) + _e283));
    C1_ = (_e275 + _e285);
    let _e287 = C0_;
    let _e293 = D;
    let _e295 = uv_5;
    let _e297 = getFromColor(((vec2<f32>(-0.792f, -0.598f) * _e293) + _e295));
    C0_ = (_e287 + _e297);
    let _e299 = C1_;
    let _e305 = D;
    let _e307 = uv_5;
    let _e309 = getToColor(((vec2<f32>(-0.792f, -0.598f) * _e305) + _e307));
    C1_ = (_e299 + _e309);
    let _e311 = C0_;
    C0_ = (_e311 / vec4(13f));
    let _e315 = C1_;
    C1_ = (_e315 / vec4(13f));
    let _e319 = C0_;
    let _e320 = C1_;
    let _e321 = T;
    return mix(_e319, _e320, vec4(_e321));
}

fn chukcut_init_globals() {
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_2: vec4<f32>;
    var a: f32;
    var local_1: vec4<f32>;

    let _e13 = U;
    progress = _e13.state.x;
    let _e17 = textureDimensions(u_from, 0i);
    dims = vec2<i32>(_e17);
    let _e20 = dims;
    let _e23 = dims;
    ratio = (f32(_e20.x) / f32(max(_e23.y, 1i)));
    let _e32 = U.params[0];
    blurSize = _e32.x;
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
    let _e24 = o_color;
    return FragmentOutput(_e24);
}
