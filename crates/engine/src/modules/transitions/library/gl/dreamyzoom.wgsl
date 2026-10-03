// DreamyZoom, from gl-transitions (https://github.com/gl-transitions/gl-transitions).
// Author: Zeh Fernando
// License: MIT
// Definitions --------
// Transition parameters --------
// In degrees
// Multiplier
// The code proper --------
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
var<private> rotation: f32;
var<private> scale: f32;

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
    var local: f32;
    var phase: f32;
    var local_1: f32;
    var angleOffset: f32;
    var local_2: f32;
    var newScale: f32;
    var center: vec2<f32> = vec2<f32>(0f, 0f);
    var assumedCenter: vec2<f32> = vec2<f32>(0.5f, 0.5f);
    var p: vec2<f32>;
    var angle: f32;
    var dist: f32;
    var local_3: vec4<f32>;
    var c_2: vec4<f32>;
    var local_4: f32;

    uv_5 = uv_4;
    let _e16 = progress;
    if (_e16 < 0.5f) {
        let _e19 = progress;
        local = (_e19 * 2f);
    } else {
        let _e22 = progress;
        local = ((_e22 - 0.5f) * 2f);
    }
    let _e28 = local;
    phase = _e28;
    let _e30 = progress;
    if (_e30 < 0.5f) {
        let _e34 = rotation;
        let _e37 = phase;
        local_1 = mix(0f, (_e34 * 0.03926991f), _e37);
    } else {
        let _e39 = rotation;
        let _e44 = phase;
        local_1 = mix((-(_e39) * 0.03926991f), 0f, _e44);
    }
    let _e47 = local_1;
    angleOffset = _e47;
    let _e49 = progress;
    if (_e49 < 0.5f) {
        let _e53 = scale;
        let _e54 = phase;
        local_2 = mix(1f, _e53, _e54);
    } else {
        let _e56 = scale;
        let _e58 = phase;
        local_2 = mix(_e56, 1f, _e58);
    }
    let _e61 = local_2;
    newScale = _e61;
    let _e73 = uv_5;
    let _e79 = newScale;
    let _e82 = ratio;
    p = (((_e73.xy - vec2<f32>(0.5f, 0.5f)) / vec2(_e79)) * vec2<f32>(_e82, 1f));
    let _e87 = p;
    let _e89 = p;
    let _e92 = angleOffset;
    angle = (atan2(_e87.y, _e89.x) + _e92);
    let _e95 = center;
    let _e96 = p;
    dist = distance(_e95, _e96);
    let _e100 = angle;
    let _e102 = dist;
    let _e104 = ratio;
    p.x = (((cos(_e100) * _e102) / _e104) + 0.5f);
    let _e109 = angle;
    let _e111 = dist;
    p.y = ((sin(_e109) * _e111) + 0.5f);
    let _e115 = progress;
    if (_e115 < 0.5f) {
        let _e118 = p;
        let _e119 = getFromColor(_e118);
        local_3 = _e119;
    } else {
        let _e120 = p;
        let _e121 = getToColor(_e120);
        local_3 = _e121;
    }
    let _e123 = local_3;
    c_2 = _e123;
    let _e125 = c_2;
    let _e126 = progress;
    if (_e126 < 0.5f) {
        let _e131 = phase;
        local_4 = mix(0f, 1f, _e131);
    } else {
        let _e135 = phase;
        local_4 = mix(1f, 0f, _e135);
    }
    let _e138 = local_4;
    return (_e125 + vec4(_e138));
}

fn chukcut_init_globals() {
    return;
}

fn main_1() {
    var dims: vec2<i32>;
    var c_3: vec4<f32>;
    var a: f32;
    var local_5: vec4<f32>;

    let _e14 = U;
    progress = _e14.state.x;
    let _e18 = textureDimensions(u_from, 0i);
    dims = vec2<i32>(_e18);
    let _e21 = dims;
    let _e24 = dims;
    ratio = (f32(_e21.x) / f32(max(_e24.y, 1i)));
    let _e33 = U.params[0];
    rotation = _e33.x;
    let _e38 = U.params[1];
    scale = _e38.x;
    chukcut_init_globals();
    let _e40 = v_uv_1;
    let _e43 = v_uv_1;
    let _e47 = transition(vec2<f32>(_e40.x, (1f - _e43.y)));
    c_3 = _e47;
    let _e49 = c_3;
    a = clamp(_e49.w, 0f, 1f);
    let _e55 = a;
    if (_e55 > 0.00001f) {
        let _e58 = c_3;
        let _e60 = a;
        let _e62 = (_e58.xyz / vec3(_e60));
        let _e63 = a;
        local_5 = vec4<f32>(_e62.x, _e62.y, _e62.z, _e63);
    } else {
        local_5 = vec4(0f);
    }
    let _e71 = local_5;
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
