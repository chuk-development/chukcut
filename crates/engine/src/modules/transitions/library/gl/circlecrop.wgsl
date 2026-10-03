// CircleCrop, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: fkuteken
// License: MIT
// ported by gre from https://gist.github.com/fkuteken/f63e3009c1143950dee9063c3b83fb88
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
var<private> ratio2_: vec2<f32>;
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
    var dist: f32;
    var local: vec4<f32>;

    p_1 = p;
    let _e17 = p_1;
    let _e22 = ratio2_;
    dist = length(((vec2<f32>(_e17) - vec2(0.5f)) * _e22));
    let _e26 = progress;
    if (_e26 < 0.5f) {
        let _e29 = p_1;
        let _e30 = getFromColor(_e29);
        local = _e30;
    } else {
        let _e31 = p_1;
        let _e32 = getToColor(_e31);
        local = _e32;
    }
    let _e34 = local;
    let _e35 = bgcolor;
    let _e36 = s;
    let _e37 = dist;
    return mix(_e34, _e35, vec4(step(_e36, _e37)));
}

fn chukcut_init_globals() {
    let _e17 = ratio;
    ratio2_ = vec2<f32>(1f, (1f / _e17));
    let _e21 = progress;
    s = pow((2f * abs((_e21 - 0.5f))), 3f);
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_2: vec4<f32>;
    var a: f32;
    var local_1: vec4<f32>;

    let _e15 = U;
    progress = _e15.state.x;
    let _e19 = textureDimensions(u_from, 0i);
    dims = vec2<i32>(_e19);
    let _e22 = dims;
    let _e25 = dims;
    ratio = (f32(_e22.x) / f32(max(_e25.y, 1i)));
    let _e34 = U.params[0];
    bgcolor = _e34;
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
        local_1 = vec4<f32>(_e57.x, _e57.y, _e57.z, _e58);
    } else {
        local_1 = vec4(0f);
    }
    let _e66 = local_1;
    o_color = _e66;
    return;
}

@fragment 
fn main(@location(0) v_uv: vec2<f32>) -> FragmentOutput {
    v_uv_1 = v_uv;
    main_1();
    let _e28 = o_color;
    return FragmentOutput(_e28);
}
