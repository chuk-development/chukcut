// directional-easing, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: Max Plotnikov
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
var<private> direction: vec2<f32>;

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
    var easing: f32;
    var p: vec2<f32>;
    var f: vec2<f32>;

    uv_5 = uv_4;
    let _e16 = progress;
    let _e18 = progress;
    easing = sqrt(((2f - _e16) * _e18));
    let _e22 = uv_5;
    let _e23 = easing;
    let _e24 = direction;
    p = (_e22 + (_e23 * sign(_e24)));
    let _e29 = p;
    f = fract(_e29);
    let _e32 = f;
    let _e33 = getToColor(_e32);
    let _e34 = f;
    let _e35 = getFromColor(_e34);
    let _e37 = p;
    let _e40 = p;
    let _e46 = p;
    let _e50 = p;
    return mix(_e33, _e35, vec4((((step(0f, _e37.y) * step(_e40.y, 1f)) * step(0f, _e46.x)) * step(_e50.x, 1f))));
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
    direction = _e32.xy;
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
