// wind, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: gre
// License: MIT
// Custom parameters
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
var<private> size: f32;
var<private> reversed: bool;

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

fn rand(co: vec2<f32>) -> f32 {
    var co_1: vec2<f32>;

    co_1 = co;
    let _e16 = co_1;
    return fract((sin(dot(_e16.xy, vec2<f32>(12.9898f, 78.233f))) * 43758.547f));
}

fn transition(uv_4: vec2<f32>) -> vec4<f32> {
    var uv_5: vec2<f32>;
    var local: f32;
    var x: f32;
    var r: f32;
    var m: f32;

    uv_5 = uv_4;
    let _e16 = reversed;
    if _e16 {
        let _e18 = uv_5;
        local = (1f - _e18.x);
    } else {
        let _e21 = uv_5;
        local = _e21.x;
    }
    let _e24 = local;
    x = _e24;
    let _e27 = uv_5;
    let _e31 = rand(vec2<f32>(0f, _e27.y));
    r = _e31;
    let _e34 = size;
    let _e36 = x;
    let _e38 = size;
    let _e41 = size;
    let _e42 = r;
    let _e45 = progress;
    let _e47 = size;
    m = smoothstep(0f, -(_e34), (((_e36 * (1f - _e38)) + (_e41 * _e42)) - (_e45 * (1f + _e47))));
    let _e53 = uv_5;
    let _e54 = getFromColor(_e53);
    let _e55 = uv_5;
    let _e56 = getToColor(_e55);
    let _e57 = m;
    return mix(_e54, _e56, vec4(_e57));
}

fn chukcut_init_globals() {
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_2: vec4<f32>;
    var a: f32;
    var local_1: vec4<f32>;

    let _e14 = U;
    progress = _e14.state.x;
    let _e18 = textureDimensions(u_from, 0i);
    dims = vec2<i32>(_e18);
    let _e21 = dims;
    let _e24 = dims;
    ratio = (f32(_e21.x) / f32(max(_e24.y, 1i)));
    let _e33 = U.params[0];
    size = _e33.x;
    let _e38 = U.params[1];
    reversed = (_e38.x > 0.5f);
    chukcut_init_globals();
    let _e42 = v_uv_1;
    let _e45 = v_uv_1;
    let _e49 = transition(vec2<f32>(_e42.x, (1f - _e45.y)));
    c_2 = _e49;
    let _e51 = c_2;
    a = clamp(_e51.w, 0f, 1f);
    let _e57 = a;
    if (_e57 > 0.00001f) {
        let _e60 = c_2;
        let _e62 = a;
        let _e64 = (_e60.xyz / vec3(_e62));
        let _e65 = a;
        local_1 = vec4<f32>(_e64.x, _e64.y, _e64.z, _e65);
    } else {
        local_1 = vec4(0f);
    }
    let _e73 = local_1;
    o_color = _e73;
    return;
}

@fragment 
fn main(@location(0) v_uv: vec2<f32>) -> FragmentOutput {
    v_uv_1 = v_uv;
    main_1();
    let _e26 = o_color;
    return FragmentOutput(_e26);
}
