// static_wipe, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: Ben Lucas
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
var<private> u_transitionUpToDown: bool;
var<private> u_max_static_span: f32;

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

fn rnd(st: vec2<f32>) -> f32 {
    var st_1: vec2<f32>;

    st_1 = st;
    let _e16 = st_1;
    return fract((sin(dot(_e16.xy, vec2<f32>(10f, 70f))) * 12345.545f));
}

fn transition(uv_4: vec2<f32>) -> vec4<f32> {
    var uv_5: vec2<f32>;
    var span: f32;
    var local: f32;
    var transitionEdge: f32;
    var mixRatio: f32;
    var transitionMix: vec4<f32>;
    var noiseEnvelope: f32;
    var noise: vec4<f32>;

    uv_5 = uv_4;
    let _e16 = u_max_static_span;
    let _e18 = progress;
    span = (_e16 * pow(sin((3.1415927f * _e18)), 0.5f));
    let _e25 = u_transitionUpToDown;
    if _e25 {
        let _e27 = uv_5;
        local = (1f - _e27.y);
    } else {
        let _e30 = uv_5;
        local = _e30.y;
    }
    let _e33 = local;
    transitionEdge = _e33;
    let _e36 = progress;
    let _e37 = transitionEdge;
    mixRatio = (1f - step(_e36, _e37));
    let _e41 = uv_5;
    let _e42 = getFromColor(_e41);
    let _e43 = uv_5;
    let _e44 = getToColor(_e43);
    let _e45 = mixRatio;
    transitionMix = mix(_e42, _e44, vec4(_e45));
    let _e49 = progress;
    let _e50 = span;
    let _e52 = progress;
    let _e53 = transitionEdge;
    let _e56 = progress;
    let _e57 = progress;
    let _e58 = span;
    let _e60 = transitionEdge;
    noiseEnvelope = (smoothstep((_e49 - _e50), _e52, _e53) * (1f - smoothstep(_e56, (_e57 + _e58), _e60)));
    let _e65 = uv_5;
    let _e67 = progress;
    let _e70 = rnd((_e65 * (1f + _e67)));
    let _e71 = vec3(_e70);
    noise = vec4<f32>(_e71.x, _e71.y, _e71.z, 1f);
    let _e78 = transitionMix;
    let _e79 = noise;
    let _e80 = noiseEnvelope;
    return mix(_e78, _e79, vec4(_e80));
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
    u_transitionUpToDown = (_e33.x > 0.5f);
    let _e40 = U.params[1];
    u_max_static_span = _e40.x;
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
