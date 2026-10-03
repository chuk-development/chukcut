// colorphase, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: gre
// License: MIT
// Usage: fromStep and toStep must be in [0.0, 1.0] range
// and all(fromStep) must be < all(toStep)
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
var<private> fromStep: vec4<f32>;
var<private> toStep: vec4<f32>;

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
    var a: vec4<f32>;
    var b: vec4<f32>;

    uv_5 = uv_4;
    let _e16 = uv_5;
    let _e17 = getFromColor(_e16);
    a = _e17;
    let _e19 = uv_5;
    let _e20 = getToColor(_e19);
    b = _e20;
    let _e22 = a;
    let _e23 = b;
    let _e24 = fromStep;
    let _e25 = toStep;
    let _e26 = progress;
    return mix(_e22, _e23, smoothstep(_e24, _e25, vec4(_e26)));
}

fn chukcut_init_globals() {
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_2: vec4<f32>;
    var a_1: f32;
    var local: vec4<f32>;

    let _e14 = U;
    progress = _e14.state.x;
    let _e18 = textureDimensions(u_from, 0i);
    dims = vec2<i32>(_e18);
    let _e21 = dims;
    let _e24 = dims;
    ratio = (f32(_e21.x) / f32(max(_e24.y, 1i)));
    let _e33 = U.params[0];
    fromStep = _e33;
    let _e37 = U.params[1];
    toStep = _e37;
    chukcut_init_globals();
    let _e38 = v_uv_1;
    let _e41 = v_uv_1;
    let _e45 = transition(vec2<f32>(_e38.x, (1f - _e41.y)));
    c_2 = _e45;
    let _e47 = c_2;
    a_1 = clamp(_e47.w, 0f, 1f);
    let _e53 = a_1;
    if (_e53 > 0.00001f) {
        let _e56 = c_2;
        let _e58 = a_1;
        let _e60 = (_e56.xyz / vec3(_e58));
        let _e61 = a_1;
        local = vec4<f32>(_e60.x, _e60.y, _e60.z, _e61);
    } else {
        local = vec4(0f);
    }
    let _e69 = local;
    o_color = _e69;
    return;
}

@fragment 
fn main(@location(0) v_uv: vec2<f32>) -> FragmentOutput {
    v_uv_1 = v_uv;
    main_1();
    let _e26 = o_color;
    return FragmentOutput(_e26);
}
