// WaterDrop, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: Paweł Płóciennik
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
var<private> amplitude: f32;
var<private> speed: f32;

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
    var dir: vec2<f32>;
    var dist: f32;
    var offset: vec2<f32>;

    p_1 = p;
    let _e16 = p_1;
    dir = (_e16 - vec2(0.5f));
    let _e21 = dir;
    dist = length(_e21);
    let _e24 = dist;
    let _e25 = progress;
    if (_e24 > _e25) {
        {
            let _e27 = p_1;
            let _e28 = getFromColor(_e27);
            let _e29 = p_1;
            let _e30 = getToColor(_e29);
            let _e31 = progress;
            return mix(_e28, _e30, vec4(_e31));
        }
    } else {
        {
            let _e34 = dir;
            let _e35 = dist;
            let _e36 = amplitude;
            let _e38 = progress;
            let _e39 = speed;
            offset = (_e34 * sin(((_e35 * _e36) - (_e38 * _e39))));
            let _e45 = p_1;
            let _e46 = offset;
            let _e48 = getFromColor((_e45 + _e46));
            let _e49 = p_1;
            let _e50 = getToColor(_e49);
            let _e51 = progress;
            return mix(_e48, _e50, vec4(_e51));
        }
    }
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
    amplitude = _e33.x;
    let _e38 = U.params[1];
    speed = _e38.x;
    chukcut_init_globals();
    let _e40 = v_uv_1;
    let _e43 = v_uv_1;
    let _e47 = transition(vec2<f32>(_e40.x, (1f - _e43.y)));
    c_2 = _e47;
    let _e49 = c_2;
    a = clamp(_e49.w, 0f, 1f);
    let _e55 = a;
    if (_e55 > 0.00001f) {
        let _e58 = c_2;
        let _e60 = a;
        let _e62 = (_e58.xyz / vec3(_e60));
        let _e63 = a;
        local = vec4<f32>(_e62.x, _e62.y, _e62.z, _e63);
    } else {
        local = vec4(0f);
    }
    let _e71 = local;
    o_color = _e71;
    return;
}

@fragment 
fn main(@location(0) v_uv: vec2<f32>) -> FragmentOutput {
    v_uv_1 = v_uv;
    main_1();
    let _e26 = o_color;
    return FragmentOutput(_e26);
}
