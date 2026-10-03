// PolkaDotsCurtain, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: bobylito
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

const SQRT_2_: f32 = 1.4142135f;

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
var<private> dots: f32;
var<private> center: vec2<f32>;

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
    var nextImage: bool;
    var local: vec4<f32>;

    uv_5 = uv_4;
    let _e17 = uv_5;
    let _e18 = dots;
    let _e25 = progress;
    let _e26 = uv_5;
    let _e27 = center;
    nextImage = (distance(fract((_e17 * _e18)), vec2<f32>(0.5f, 0.5f)) < (_e25 / distance(_e26, _e27)));
    let _e32 = nextImage;
    if _e32 {
        let _e33 = uv_5;
        let _e34 = getToColor(_e33);
        local = _e34;
    } else {
        let _e35 = uv_5;
        let _e36 = getFromColor(_e35);
        local = _e36;
    }
    let _e38 = local;
    return _e38;
}

fn chukcut_init_globals() {
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
    dots = _e34.x;
    let _e39 = U.params[1];
    center = _e39.xy;
    chukcut_init_globals();
    let _e41 = v_uv_1;
    let _e44 = v_uv_1;
    let _e48 = transition(vec2<f32>(_e41.x, (1f - _e44.y)));
    c_2 = _e48;
    let _e50 = c_2;
    a = clamp(_e50.w, 0f, 1f);
    let _e56 = a;
    if (_e56 > 0.00001f) {
        let _e59 = c_2;
        let _e61 = a;
        let _e63 = (_e59.xyz / vec3(_e61));
        let _e64 = a;
        local_1 = vec4<f32>(_e63.x, _e63.y, _e63.z, _e64);
    } else {
        local_1 = vec4(0f);
    }
    let _e72 = local_1;
    o_color = _e72;
    return;
}

@fragment 
fn main(@location(0) v_uv: vec2<f32>) -> FragmentOutput {
    v_uv_1 = v_uv;
    main_1();
    let _e28 = o_color;
    return FragmentOutput(_e28);
}
